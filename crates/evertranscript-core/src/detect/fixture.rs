//! A scripted timeline of things the machine did.
//!
//! The other half of the DetectionSource seam. Every Auto-Record decision is
//! exercised by replaying one of these instead of holding a meeting, which
//! is the only way the policy's expensive cases — the device swap that must
//! not split a Meeting, the manual Stop that must not be overruled — get
//! tested at all.
//!
//! **The fragment rule.** M1 shipped a chunker that was correct against
//! whole-file fixtures and discarded every sample a live microphone
//! produced, because fixtures arrived whole and hardware arrived in pieces.
//! The same trap is set here: a timeline that jumps from one interesting
//! moment to the next will pass a policy that mishandles the quiet time
//! between them. [`Timeline::fragmented`] fills that quiet with ticks, and
//! the policy suite runs the important timelines both ways.

use anyhow::Result;
use tokio::sync::mpsc;

use super::AppIdentity;
use super::CalendarEvent;
use super::DetectionEvent;
use super::DetectionInstant;
use super::DetectionSource;

/// Builds a timeline by describing what happened, and when.
#[derive(Debug, Clone, Default)]
pub struct Timeline {
    events: Vec<DetectionEvent>,
    now: DetectionInstant,
}

impl Timeline {
    pub fn new() -> Self {
        Self::default()
    }

    /// Advances the clock without anything happening.
    pub fn wait(mut self, millis: u64) -> Self {
        self.now = self.now.plus_millis(millis);
        self
    }

    pub fn app_active(mut self, app: &str) -> Self {
        self.events.push(DetectionEvent::AppActive {
            at: self.now,
            app: AppIdentity::bare(app),
        });
        self
    }

    pub fn app_gone(mut self, app: &str) -> Self {
        self.events.push(DetectionEvent::AppGone {
            at: self.now,
            app: AppIdentity::bare(app),
        });
        self
    }

    pub fn mic_held(mut self, app: &str) -> Self {
        self.events.push(DetectionEvent::MicHeld {
            at: self.now,
            app: AppIdentity::bare(app),
        });
        self
    }

    pub fn mic_released(mut self, app: &str) -> Self {
        self.events.push(DetectionEvent::MicReleased {
            at: self.now,
            app: AppIdentity::bare(app),
        });
        self
    }

    pub fn calendar_started(mut self, id: &str, title: &str, runs_for_ms: u64) -> Self {
        self.events.push(DetectionEvent::CalendarEventStarted {
            at: self.now,
            event: CalendarEvent {
                id: id.to_string(),
                title: title.to_string(),
                attendees: Vec::new(),
                scheduled_end: Some(self.now.plus_millis(runs_for_ms)),
            },
        });
        self
    }

    pub fn calendar_ended(mut self, id: &str) -> Self {
        self.events.push(DetectionEvent::CalendarEventEnded {
            at: self.now,
            id: id.to_string(),
        });
        self
    }

    /// A bare passage of time the policy is asked about.
    pub fn tick(mut self) -> Self {
        self.events.push(DetectionEvent::Tick { at: self.now });
        self
    }

    pub fn events(&self) -> &[DetectionEvent] {
        &self.events
    }

    pub fn into_events(self) -> Vec<DetectionEvent> {
        self.events
    }

    /// The same timeline, with the silence between events filled by ticks.
    ///
    /// This is the fragment rule made usable. A policy tested only against
    /// the sparse form is a policy tested only against the shape a fixture
    /// happens to produce — and the live source produces the other one.
    pub fn fragmented(self, every_ms: u64) -> Vec<DetectionEvent> {
        assert!(every_ms > 0, "a granularity of zero would never advance");
        let mut out: Vec<DetectionEvent> = Vec::new();
        let mut clock = DetectionInstant::ZERO;
        for event in self.events.clone() {
            while clock.plus_millis(every_ms) <= event.at() {
                clock = clock.plus_millis(every_ms);
                out.push(DetectionEvent::Tick { at: clock });
            }
            clock = event.at();
            out.push(event);
        }
        // Fill the tail too. `wait(30_000)` at the end of a timeline reads
        // as "and then half a minute passed", and a fragmented form that
        // stopped at the last scripted event would silently drop it —
        // leaving a continuity window that never expires because nothing
        // ever asked again.
        while clock.plus_millis(every_ms) <= self.now {
            clock = clock.plus_millis(every_ms);
            out.push(DetectionEvent::Tick { at: clock });
        }
        out
    }
}

/// Plays a timeline as fast as the consumer accepts it.
///
/// Time comes from the script, never from real elapsed time, so an
/// afternoon of meetings runs in milliseconds and always the same way.
pub struct FixtureDetectionSource {
    events: Vec<DetectionEvent>,
    handle: Option<tokio::task::JoinHandle<()>>,
    /// Fires once the whole timeline has been delivered, so a test awaits a
    /// finished script instead of sleeping and hoping.
    finished: Option<tokio::sync::oneshot::Sender<()>>,
}

impl FixtureDetectionSource {
    pub fn new(events: Vec<DetectionEvent>) -> Self {
        Self {
            events,
            handle: None,
            finished: None,
        }
    }

    pub fn from_timeline(timeline: Timeline) -> Self {
        Self::new(timeline.into_events())
    }

    pub fn with_completion(
        events: Vec<DetectionEvent>,
    ) -> (Self, tokio::sync::oneshot::Receiver<()>) {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let mut source = Self::new(events);
        source.finished = Some(sender);
        (source, receiver)
    }
}

impl DetectionSource for FixtureDetectionSource {
    fn start(&mut self, events: mpsc::Sender<DetectionEvent>) -> Result<()> {
        let script = std::mem::take(&mut self.events);
        let finished = self.finished.take();
        self.handle = Some(tokio::spawn(async move {
            for event in script {
                if events.send(event).await.is_err() {
                    break;
                }
            }
            if let Some(finished) = finished {
                let _ = finished.send(());
            }
        }));
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }

    fn describe(&self) -> String {
        "fixture".to_string()
    }
}

/// The timelines this milestone argues about, named once so every test
/// argues about the same ones.
pub mod timelines {
    use super::Timeline;

    /// Zoom active, microphone hot, talking, then done.
    pub fn clean_meeting() -> Timeline {
        Timeline::new()
            .app_active("us.zoom.xos")
            .mic_held("us.zoom.xos")
            .wait(600_000)
            .mic_released("us.zoom.xos")
            .wait(30_000)
            .tick()
    }

    /// The AirPods swap. The microphone drops for eight seconds inside the
    /// continuity window and comes back: one Meeting, not two.
    pub fn device_swap_mid_meeting() -> Timeline {
        Timeline::new()
            .app_active("us.zoom.xos")
            .mic_held("us.zoom.xos")
            .wait(120_000)
            .mic_released("us.zoom.xos")
            .wait(8_000)
            .mic_held("us.zoom.xos")
            .wait(120_000)
            .mic_released("us.zoom.xos")
            .wait(30_000)
            .tick()
    }

    /// An idle Zoom window. Nothing should ever record.
    pub fn app_active_but_silent() -> Timeline {
        Timeline::new()
            .app_active("us.zoom.xos")
            .wait(600_000)
            .tick()
    }

    /// Dictation. A hot microphone that is not a meeting.
    pub fn mic_held_by_stranger() -> Timeline {
        Timeline::new()
            .app_active("com.superwhisper")
            .mic_held("com.superwhisper")
            .wait(120_000)
            .mic_released("com.superwhisper")
            .wait(30_000)
            .tick()
    }

    /// Two meetings in the same app, half an hour apart. The second one
    /// must record — this is the timeline that catches suppression which
    /// never expires.
    pub fn back_to_back_meetings() -> Timeline {
        Timeline::new()
            .app_active("us.zoom.xos")
            .mic_held("us.zoom.xos")
            .wait(600_000)
            .mic_released("us.zoom.xos")
            .wait(1_800_000)
            .mic_held("us.zoom.xos")
            .wait(600_000)
            .mic_released("us.zoom.xos")
            .wait(30_000)
            .tick()
    }

    /// Detection comes online while a meeting is already under way: the
    /// microphone is already held when the first event arrives.
    pub fn joined_late() -> Timeline {
        Timeline::new()
            .mic_held("us.zoom.xos")
            .app_active("us.zoom.xos")
            .wait(300_000)
            .mic_released("us.zoom.xos")
            .wait(30_000)
            .tick()
    }

    /// A calendar event starts and nobody ever joins.
    pub fn armed_but_never_triggered() -> Timeline {
        Timeline::new()
            .calendar_started("evt-1", "Weekly sync", 1_800_000)
            .wait(600_000)
            .tick()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragmenting_preserves_the_scripted_events_and_their_order() {
        let sparse = timelines::device_swap_mid_meeting();
        let scripted: Vec<_> = sparse.events().to_vec();
        let fragmented = sparse.clone().fragmented(1_000);

        let kept: Vec<_> = fragmented
            .iter()
            .filter(|event| !matches!(event, DetectionEvent::Tick { .. }))
            .cloned()
            .collect();
        let expected: Vec<_> = scripted
            .into_iter()
            .filter(|event| !matches!(event, DetectionEvent::Tick { .. }))
            .collect();
        assert_eq!(
            kept, expected,
            "fragmenting must not lose or reorder events"
        );
    }

    #[test]
    fn fragmenting_fills_the_silence_the_sparse_form_skips() {
        // The whole point: a policy tested only against the sparse form is
        // tested only against the shape a fixture happens to produce.
        let sparse = timelines::clean_meeting();
        let sparse_count = sparse.events().len();
        let fragmented = sparse.fragmented(1_000);
        assert!(
            fragmented.len() > sparse_count * 10,
            "600 s of meeting at 1 s granularity should be hundreds of events, got {}",
            fragmented.len()
        );
    }

    #[test]
    fn a_fragmented_timeline_never_goes_backwards() {
        let events = timelines::back_to_back_meetings().fragmented(997);
        let mut previous = DetectionInstant::ZERO;
        for event in &events {
            assert!(
                event.at() >= previous,
                "time went backwards at {event:?} (previous {previous:?})"
            );
            previous = event.at();
        }
    }
}
