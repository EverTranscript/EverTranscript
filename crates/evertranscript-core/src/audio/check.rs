//! Answering "can this machine actually record?" by recording.
//!
//! Asking the operating system does not work, which is the whole reason this
//! module exists. macOS grants the system-audio tap whether or not capture
//! was allowed, and a refused one delivers silence forever without ever
//! failing — so a permission query returns "granted" for a leg that will
//! never carry a sample. The only honest question is what arrives.
//!
//! Shared rather than duplicated. The CLI ran this check for a long time and
//! the Client did not, while the Client's own onboarding copy promised it —
//! two surfaces, one of them lying. They now compute the same verdict from
//! the same recording, and differ only in the words they print.

use std::time::Duration;
use std::time::Instant;

use evertranscript_protocol::AudioChannel;
use evertranscript_protocol::AudioCheckResponse;
use evertranscript_protocol::AudioCheckVerdict;
use evertranscript_protocol::AudioLegReport;
use evertranscript_protocol::AudioLegState;

use super::AudioSource;
use super::CaptureClock;
use super::CaptureEvent;
use super::live::LiveSource;

/// How long to listen when the caller does not say.
pub const DEFAULT_SECONDS: u64 = 20;

/// Records for `seconds` and reports what arrived on each leg.
pub async fn run(seconds: u64) -> AudioCheckResponse {
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(4096);
    let mut source = LiveSource::new();
    let started = source.start(CaptureClock::start(), events_tx);

    // Whether anything played during the window. Without it, "captured, but
    // all of it silent" reads as a permission problem even when the Operator
    // simply had nothing playing — the same confusion the Core's own refusal
    // check used to make (DECISIONS Q9).
    //
    // `None` means this platform cannot say, and that is not the same as
    // "nothing played": reading it as `false` would make every silent system
    // leg on such a platform report the untested wording, which is the
    // confusion in the other direction.
    let mut heard_playback: Option<bool> = None;
    let could_not_start = match &started {
        Err(error) => Some(format!("{error:#}")),
        Ok(_) => {
            let until = Instant::now() + Duration::from_secs(seconds);
            while Instant::now() < until {
                if let Some(active) = super::system::output_is_active() {
                    heard_playback = Some(heard_playback.unwrap_or(false) || active);
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            None
        }
    };
    source.stop();

    // Two legs, so two counters; AudioChannel is a protocol type and not
    // worth making map-keyable for this.
    let (mut mic, mut system) = ((0u64, 0.0f32), (0u64, 0.0f32));
    let mut unavailable: Vec<(AudioChannel, String)> = Vec::new();
    while let Ok(event) = events_rx.try_recv() {
        match event {
            CaptureEvent::Frame(frame) => {
                let peak = frame
                    .samples
                    .iter()
                    .fold(0.0f32, |max, sample| max.max(sample.abs()));
                let entry = match frame.channel {
                    AudioChannel::Mic => &mut mic,
                    AudioChannel::System => &mut system,
                };
                entry.0 += frame.duration_ms();
                entry.1 = entry.1.max(peak);
            }
            CaptureEvent::Unavailable { channel, reason }
            | CaptureEvent::Degraded { channel, reason } => unavailable.push((channel, reason)),
            CaptureEvent::StreamFailed { channel, error } => unavailable.push((channel, error)),
            CaptureEvent::DeviceChanged { .. } => {}
        }
    }

    let legs = [(AudioChannel::Mic, mic), (AudioChannel::System, system)]
        .into_iter()
        .map(|(channel, (milliseconds, peak))| AudioLegReport {
            channel,
            state: leg_state(channel, milliseconds, peak, heard_playback),
            milliseconds,
            peak,
            reason: unavailable
                .iter()
                .find(|(other, _)| *other == channel)
                .map(|(_, reason)| reason.clone()),
        })
        .collect::<Vec<_>>();

    AudioCheckResponse {
        verdict: verdict(&legs),
        legs,
        could_not_start,
    }
}

/// Frames whose samples are all zero are the failure this whole check exists
/// to catch, so they do not count as a working leg.
fn leg_state(
    channel: AudioChannel,
    milliseconds: u64,
    peak: f32,
    heard_playback: Option<bool>,
) -> AudioLegState {
    if peak > 0.0 {
        return AudioLegState::Working;
    }
    // Nothing was played, so the system leg was never asked a question it
    // could answer. Only the system leg: a microphone has nothing to wait for.
    //
    // **Before the zero check, not after.** A CoreAudio process tap delivers
    // no callbacks whatsoever while nothing is playing — not silence, nothing
    // — so zero milliseconds from it is an unasked question and not a failure.
    // Ordering this after the zero check made a healthy machine report
    // "nothing captured … Meetings will record, and be marked partial", which
    // sends the Operator to System Settings over a tap doing exactly what it
    // should. The recorder's `judge_silent_leg` already drew this line; the
    // two now agree.
    if channel == AudioChannel::System && heard_playback == Some(false) {
        return AudioLegState::NotTested;
    }
    if milliseconds == 0 {
        return AudioLegState::NothingCaptured;
    }
    AudioLegState::Silent
}

fn verdict(legs: &[AudioLegReport]) -> AudioCheckVerdict {
    let count = |wanted: AudioLegState| legs.iter().filter(|leg| leg.state == wanted).count();
    match (
        count(AudioLegState::Working),
        count(AudioLegState::NotTested),
    ) {
        (2, _) => AudioCheckVerdict::BothLegsWork,
        (1, 1) => AudioCheckVerdict::MicrophoneWorksOtherUntested,
        (1, _) => AudioCheckVerdict::OneLegWorks,
        (0, 0) => AudioCheckVerdict::NothingCaptured,
        _ => AudioCheckVerdict::NothingTested,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leg(channel: AudioChannel, state: AudioLegState) -> AudioLegReport {
        AudioLegReport {
            channel,
            state,
            milliseconds: 1000,
            peak: 0.0,
            reason: None,
        }
    }

    #[test]
    fn silence_while_nothing_played_is_untested_rather_than_refused() {
        // The distinction Q9 was about. Calling this "silent" sends the
        // Operator to System Settings to fix a permission that is fine.
        assert_eq!(
            leg_state(AudioChannel::System, 5000, 0.0, Some(false)),
            AudioLegState::NotTested
        );
        // But a microphone has nothing to wait for: silence from it is
        // silence, whatever was or was not playing.
        assert_eq!(
            leg_state(AudioChannel::Mic, 5000, 0.0, Some(false)),
            AudioLegState::Silent
        );
    }

    #[test]
    fn silence_on_a_platform_that_cannot_say_is_not_excused() {
        // `None` is "this platform cannot tell", which must not be read as
        // "nothing played" — that would excuse every refused system leg.
        assert_eq!(
            leg_state(AudioChannel::System, 5000, 0.0, None),
            AudioLegState::Silent
        );
    }

    #[test]
    fn a_leg_that_delivered_nothing_is_not_the_same_as_one_that_delivered_silence() {
        assert_eq!(
            leg_state(AudioChannel::Mic, 0, 0.0, None),
            AudioLegState::NothingCaptured
        );
    }

    #[test]
    fn an_idle_tap_delivering_nothing_is_untested_rather_than_broken() {
        // The tap produces no callbacks at all while nothing plays, so zero
        // milliseconds from it says nothing about whether it works. Reporting
        // that as a failure told a healthy machine its Meetings would be
        // partial.
        assert_eq!(
            leg_state(AudioChannel::System, 0, 0.0, Some(false)),
            AudioLegState::NotTested
        );
        // But with something playing, zero really is nothing captured.
        assert_eq!(
            leg_state(AudioChannel::System, 0, 0.0, Some(true)),
            AudioLegState::NothingCaptured
        );
        // And a microphone that delivered nothing is broken whatever was
        // playing: it should have produced frames of zeros.
        assert_eq!(
            leg_state(AudioChannel::Mic, 0, 0.0, Some(false)),
            AudioLegState::NothingCaptured
        );
    }

    #[test]
    fn one_working_leg_beside_an_untested_one_does_not_claim_the_other_failed() {
        assert_eq!(
            verdict(&[
                leg(AudioChannel::Mic, AudioLegState::Working),
                leg(AudioChannel::System, AudioLegState::NotTested),
            ]),
            AudioCheckVerdict::MicrophoneWorksOtherUntested
        );
    }

    #[test]
    fn verdicts_cover_the_rest_of_the_shapes() {
        assert_eq!(
            verdict(&[
                leg(AudioChannel::Mic, AudioLegState::Working),
                leg(AudioChannel::System, AudioLegState::Working),
            ]),
            AudioCheckVerdict::BothLegsWork
        );
        assert_eq!(
            verdict(&[
                leg(AudioChannel::Mic, AudioLegState::Working),
                leg(AudioChannel::System, AudioLegState::Silent),
            ]),
            AudioCheckVerdict::OneLegWorks
        );
        assert_eq!(
            verdict(&[
                leg(AudioChannel::Mic, AudioLegState::Silent),
                leg(AudioChannel::System, AudioLegState::Silent),
            ]),
            AudioCheckVerdict::NothingCaptured
        );
        assert_eq!(
            verdict(&[
                leg(AudioChannel::Mic, AudioLegState::NothingCaptured),
                leg(AudioChannel::System, AudioLegState::NotTested),
            ]),
            AudioCheckVerdict::NothingTested
        );
    }
}
