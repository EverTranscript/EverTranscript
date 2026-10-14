//! The churn contract: what happens when capture hardware misbehaves
//! (ADR-0029 as amended).
//!
//! The rules, and why each one is the way it is:
//!
//! - A **device change** is housekeeping, not a failure. Swapping AirPods
//!   mid-meeting must not spend the budget meant for real faults, or a few
//!   swaps would end the recording.
//! - A **stream error** spends budget. Restarting forever against broken
//!   hardware is how a "resilient" recorder burns a laptop's battery to zero
//!   producing silence.
//! - Exhausting the budget ends **that leg**, never the Meeting. The mic
//!   dying does not stop system audio, and neither stops the transcript.
//! - The **session outlives every stream.** Nothing here can create a second
//!   Meeting, which is why device churn cannot split one recording in two —
//!   the failure both open-source competitors have shipped.
//! - Failures count **consecutively**, and only proof of health — a leg that
//!   has delivered steadily since ([`CaptureEvent::Healthy`]) — earns the
//!   budget back. Time passing does not: a device that dies, comes back for a
//!   second and dies again is the same broken device.
//!
//! The consecutive count and its reset are PORTED from `fastrepl/anarlog`,
//! Copyright (c) 2023-present Fastrepl, Inc., licensed MIT:
//! `crates/supervisor/src/restart.rs` (`record_consecutive_restart`, `reset`)
//! at rev `693500220d`. See `PORTS.md`.

use std::collections::HashMap;

use evertranscript_protocol::AudioChannel;
use tracing::debug;
use tracing::info;
use tracing::warn;

use super::CaptureEvent;

/// How many consecutive restarts a leg gets before it is ended.
///
/// Three, matching the microphone's reopen ladder in `live.rs` (1s, 2s, 4s):
/// the failure that finds no delay left there is the one that exceeds this.
#[derive(Debug, Clone, Copy)]
pub struct RestartBudget {
    pub max_restarts: u32,
}

impl Default for RestartBudget {
    fn default() -> Self {
        Self { max_restarts: 3 }
    }
}

/// Tracks one leg's consecutive restarts against its budget.
#[derive(Debug, Default)]
struct RestartTracker {
    count: u32,
}

impl RestartTracker {
    /// Records a restart that counts, returning whether the leg may continue.
    fn record_consecutive_restart(&mut self, budget: &RestartBudget) -> bool {
        self.count = self.count.saturating_add(1);
        self.count <= budget.max_restarts
    }

    /// The leg has proved itself healthy since its last failure.
    fn reset(&mut self) {
        self.count = 0;
    }

    /// Backoff before the next attempt: 1s, 2s, 4s…
    fn backoff(&self) -> std::time::Duration {
        let exponent = self.count.saturating_sub(1).min(4);
        std::time::Duration::from_secs(1 << exponent)
    }
}

/// What the recorder should do about an event.
#[derive(Debug, PartialEq)]
pub enum Action {
    /// Nothing to do; the frame was consumed.
    Continue,
    /// Restart this leg after the given delay, keeping the same Meeting.
    RestartLeg {
        channel: AudioChannel,
        after: std::time::Duration,
        /// False for a device change: expected churn does not spend budget.
        counted: bool,
    },
    /// This leg is done. The Meeting continues on whatever remains.
    EndLeg {
        channel: AudioChannel,
        reason: String,
    },
    /// Record why this leg is not useful, but leave it running.
    NoteLeg {
        channel: AudioChannel,
        reason: String,
    },
}

/// Decides what happens to each capture leg.
pub struct ChurnPolicy {
    trackers: HashMap<AudioChannel, RestartTracker>,
    budget: RestartBudget,
}

impl ChurnPolicy {
    pub fn new(budget: RestartBudget) -> Self {
        Self {
            trackers: HashMap::new(),
            budget,
        }
    }

    /// Classifies one capture event.
    pub fn decide(&mut self, event: &CaptureEvent) -> Action {
        match event {
            CaptureEvent::Frame(_) => Action::Continue,

            CaptureEvent::DeviceChanged { channel } => {
                // Budget-free and immediate: the Operator swapped a device
                // and expects to keep being recorded.
                info!(?channel, "capture device changed; restarting this leg");
                Action::RestartLeg {
                    channel: *channel,
                    after: std::time::Duration::ZERO,
                    counted: false,
                }
            }

            CaptureEvent::Healthy { channel } => {
                if let Some(tracker) = self.trackers.get_mut(channel) {
                    info!(
                        ?channel,
                        "capture leg is healthy again; its budget is restored"
                    );
                    tracker.reset();
                }
                Action::Continue
            }

            CaptureEvent::StreamFailed { channel, error } => {
                let tracker = self.trackers.entry(*channel).or_default();
                let may_continue = tracker.record_consecutive_restart(&self.budget);
                if may_continue {
                    let after = tracker.backoff();
                    warn!(
                        ?channel,
                        error,
                        attempt = tracker.count,
                        "capture stream failed; restarting this leg"
                    );
                    Action::RestartLeg {
                        channel: *channel,
                        after,
                        counted: true,
                    }
                } else {
                    warn!(
                        ?channel,
                        error, "capture stream keeps failing; giving up on this leg"
                    );
                    Action::EndLeg {
                        channel: *channel,
                        reason: format!(
                            "{channel:?} capture failed {} times in a row: {error}",
                            tracker.count
                        ),
                    }
                }
            }

            CaptureEvent::Unavailable { channel, reason } => {
                // Never retried: retrying something the platform does not
                // support is noise, and the Meeting is fine without it.
                debug!(?channel, reason, "capture leg unavailable on this machine");
                Action::EndLeg {
                    channel: *channel,
                    reason: reason.clone(),
                }
            }

            CaptureEvent::Degraded { channel, reason } => {
                // Not ended, on purpose. The leg is still delivering, and a
                // diagnosis this Core cannot be certain of must not be able
                // to cost the rest of the meeting's audio.
                debug!(
                    ?channel,
                    reason, "capture leg is degraded but still attached"
                );
                Action::NoteLeg {
                    channel: *channel,
                    reason: reason.clone(),
                }
            }
        }
    }
}

impl Default for ChurnPolicy {
    fn default() -> Self {
        Self::new(RestartBudget::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(channel: AudioChannel) -> CaptureEvent {
        CaptureEvent::StreamFailed {
            channel,
            error: "device disappeared".to_string(),
        }
    }

    #[test]
    fn a_device_change_restarts_the_leg_without_spending_budget() {
        let mut policy = ChurnPolicy::default();

        // Far more swaps than the failure budget would allow.
        for _ in 0..10 {
            let action = policy.decide(&CaptureEvent::DeviceChanged {
                channel: AudioChannel::Mic,
            });
            assert_eq!(
                action,
                Action::RestartLeg {
                    channel: AudioChannel::Mic,
                    after: std::time::Duration::ZERO,
                    counted: false,
                },
                "swapping devices must never exhaust the failure budget"
            );
        }
    }

    #[test]
    fn stream_failures_back_off_and_then_give_up_on_the_leg() {
        let mut policy = ChurnPolicy::new(RestartBudget { max_restarts: 3 });

        let mut delays = Vec::new();
        for _ in 0..3 {
            match policy.decide(&failure(AudioChannel::System)) {
                Action::RestartLeg { after, counted, .. } => {
                    assert!(counted, "a real failure spends budget");
                    delays.push(after);
                }
                other => panic!("expected a restart, got {other:?}"),
            }
        }
        assert_eq!(
            delays,
            vec![
                std::time::Duration::from_secs(1),
                std::time::Duration::from_secs(2),
                std::time::Duration::from_secs(4),
            ],
            "backoff should grow rather than hammer broken hardware"
        );

        match policy.decide(&failure(AudioChannel::System)) {
            Action::EndLeg { channel, .. } => assert_eq!(channel, AudioChannel::System),
            other => panic!("the budget should be exhausted, got {other:?}"),
        }
    }

    #[test]
    fn one_legs_failures_do_not_consume_the_others_budget() {
        let mut policy = ChurnPolicy::new(RestartBudget { max_restarts: 2 });

        // Exhaust the system leg entirely.
        policy.decide(&failure(AudioChannel::System));
        policy.decide(&failure(AudioChannel::System));
        assert!(matches!(
            policy.decide(&failure(AudioChannel::System)),
            Action::EndLeg { .. }
        ));

        // The microphone must be untouched by that: losing system audio
        // cannot be allowed to take the Operator's own voice with it.
        assert!(
            matches!(
                policy.decide(&failure(AudioChannel::Mic)),
                Action::RestartLeg { .. }
            ),
            "the legs must have independent budgets"
        );
    }

    #[test]
    fn a_degraded_leg_is_noted_but_never_ended() {
        // The distinction this enum exists to draw. A refused system-audio
        // permission is a diagnosis the Core infers rather than reads, and
        // it has been wrong before (DECISIONS Q9): a meeting that simply
        // opened quietly was told its audio was incomplete. Ending the leg
        // on that made the mistake cost the rest of the meeting, so a
        // degraded leg is recorded and left running.
        let mut policy = ChurnPolicy::default();
        let action = policy.decide(&CaptureEvent::Degraded {
            channel: AudioChannel::System,
            reason: "arrives as silence".to_string(),
        });
        match action {
            Action::NoteLeg { channel, reason } => {
                assert_eq!(channel, AudioChannel::System);
                assert!(reason.contains("silence"));
            }
            other => panic!("a degraded leg must be noted, not ended, got {other:?}"),
        }
    }

    #[test]
    fn an_unavailable_leg_ends_immediately_rather_than_retrying() {
        let mut policy = ChurnPolicy::default();
        let action = policy.decide(&CaptureEvent::Unavailable {
            channel: AudioChannel::System,
            reason: "system audio capture is not implemented on this platform".to_string(),
        });
        match action {
            Action::EndLeg { channel, reason } => {
                assert_eq!(channel, AudioChannel::System);
                assert!(reason.contains("not implemented"));
            }
            other => panic!("expected the leg to end, got {other:?}"),
        }
    }

    /// Upstream's `consecutive_failures_stay_bounded_until_capture_is_healthy`.
    #[test]
    fn consecutive_failures_stay_bounded_until_the_leg_is_healthy() {
        let mut policy = ChurnPolicy::new(RestartBudget { max_restarts: 3 });
        for _ in 0..3 {
            assert!(matches!(
                policy.decide(&failure(AudioChannel::Mic)),
                Action::RestartLeg { .. }
            ));
        }
        assert!(
            matches!(
                policy.decide(&failure(AudioChannel::Mic)),
                Action::EndLeg { .. }
            ),
            "a fourth failure in a row exceeds a budget of three, however long it took"
        );

        let mut policy = ChurnPolicy::new(RestartBudget { max_restarts: 3 });
        for _ in 0..3 {
            policy.decide(&failure(AudioChannel::Mic));
        }
        assert_eq!(
            policy.decide(&CaptureEvent::Healthy {
                channel: AudioChannel::Mic
            }),
            Action::Continue
        );
        match policy.decide(&failure(AudioChannel::Mic)) {
            Action::RestartLeg { after, .. } => assert_eq!(
                after,
                std::time::Duration::from_secs(1),
                "a healthy leg starts the ladder again from the bottom"
            ),
            other => panic!("a healthy leg should get its retries back, got {other:?}"),
        }
    }
}
