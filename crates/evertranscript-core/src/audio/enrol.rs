//! Recording the Operator saying something, so their voice stops having to
//! be guessed at.
//!
//! Every other Voiceprint this product holds is inferred from a meeting
//! nobody confirmed: `diarize::operator` reads the microphone channel and
//! decides who owns the laptop. It is usually right and it is silently wrong
//! in exactly one case — somebody else in the room — because a transcript
//! that names the wrong person looks exactly like one that names the right
//! one. An enrolment is the other kind of evidence, and the only kind this
//! file produces: a person recorded themselves and said so.
//!
//! **The microphone and nothing else.** This is the one capture in the
//! product with no far end to record, so it opens no system-audio tap —
//! see [`super::AudioSource::start_microphone_only`]. Non-mic frames are
//! dropped here as well, which is belt and braces on the live path and the
//! whole assertion on a fixture one.
//!
//! What this module does *not* do is decide whether the recording is any
//! good. Silence it can see; one voice or two is a question for the
//! diarizer, and lives in `diarize::enrol` where the models are.

use std::time::Duration;
use std::time::Instant;

use evertranscript_protocol::AudioChannel;

use super::AudioSource;
use super::CaptureClock;
use super::CaptureEvent;

/// How long to listen when the caller does not say.
///
/// Forty rather than the twenty `check` uses. The check only has to prove a
/// leg is live; this has to hold enough of one voice to be an identity, and
/// `diarize::operator::MIN_OPERATOR_MS` already puts that floor at twenty
/// seconds of *voiced* audio — which nobody produces in twenty seconds of
/// wall clock, because people breathe.
///
/// **The margin is a guess and is deliberately generous.** What matters is
/// the ratio of voiced audio to wall clock, as *the diarizer* counts turns,
/// and that has not been measured on a real enrolment on real hardware. At
/// thirty seconds a normal speaker needs two thirds of the recording to be
/// speech to clear the floor; at forty, half. Ten seconds of somebody's time
/// is the cheaper side of that to be wrong on — the other side is a first-run
/// step that refuses people who did nothing wrong, which teaches them to
/// skip it. Worth narrowing once there is a measurement to narrow it with.
pub const DEFAULT_SECONDS: u64 = 40;

/// Mic audio, at the rate it was captured.
pub struct Recorded {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    /// Loudest sample seen. Zero means the leg delivered nothing but
    /// silence, which on macOS is what a refused permission looks like —
    /// the grant is reported as given and the samples are all zero.
    pub peak: f32,
}

impl Recorded {
    pub fn duration_ms(&self) -> u64 {
        if self.sample_rate == 0 {
            return 0;
        }
        self.samples.len() as u64 * 1000 / self.sample_rate as u64
    }
}

/// Records `seconds` of the microphone from `source`.
///
/// The source is passed in rather than built here, which is what makes this
/// function reachable from a test at all: `Core` hands over the same
/// `source_factory` a Meeting is started through, so a fixture reaches the
/// enrolment exactly as it reaches every other capture in the product.
///
/// Drains as it goes rather than reading the channel out afterwards: a
/// half-minute of frames is close enough to the channel's capacity that
/// "the buffer was big enough" would be a thing that quietly stopped being
/// true the day the frame size changed.
pub async fn record(mut source: Box<dyn AudioSource>, seconds: u64) -> anyhow::Result<Recorded> {
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(4096);
    source.start_microphone_only(CaptureClock::start(), events_tx)?;

    let mut samples: Vec<f32> = Vec::new();
    let mut peak = 0.0_f32;
    let mut failure: Option<String> = None;
    let until = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < until {
        tokio::time::sleep(Duration::from_millis(50)).await;
        while let Ok(event) = events_rx.try_recv() {
            match event {
                CaptureEvent::Frame(frame) if frame.channel == AudioChannel::Mic => {
                    peak = frame
                        .samples
                        .iter()
                        .fold(peak, |max, sample| max.max(sample.abs()));
                    samples.extend_from_slice(&frame.samples);
                }
                CaptureEvent::Frame(_) => {}
                CaptureEvent::Unavailable { reason, .. }
                | CaptureEvent::Degraded { reason, .. } => {
                    failure.get_or_insert(reason);
                }
                CaptureEvent::StreamFailed { error, .. } => {
                    failure.get_or_insert(error);
                }
                CaptureEvent::DeviceChanged { .. } => {}
            }
        }
        if failure.is_some() {
            break;
        }
    }
    source.stop();

    if let Some(reason) = failure {
        anyhow::bail!("the microphone stopped during the recording: {reason}");
    }
    Ok(Recorded {
        samples,
        sample_rate: super::SAMPLE_RATE,
        peak,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::fixture::FixtureSource;
    use crate::audio::fixture::Step;

    /// How many samples `ms` of scripted audio is worth.
    fn samples_in(ms: u64) -> usize {
        (super::super::SAMPLE_RATE as u64 * ms / 1000) as usize
    }

    #[tokio::test]
    async fn the_recording_is_the_microphone_and_only_the_microphone() {
        // The far end is louder than the near one on purpose. If a system
        // frame ever reached the buffer the peak would be 0.9 and the
        // sample count would be wrong — which is the failure this whole
        // module's "microphone and nothing else" claim would take, and it
        // would take it silently.
        let source = FixtureSource::new(vec![
            Step::audio(AudioChannel::Mic, 300, 0.5),
            Step::audio(AudioChannel::System, 300, -0.9),
            Step::audio(AudioChannel::Mic, 200, -0.25),
        ]);

        let recorded = record(Box::new(source), 1).await.expect("record");

        assert_eq!(recorded.samples.len(), samples_in(500));
        assert_eq!(recorded.sample_rate, super::super::SAMPLE_RATE);
        assert_eq!(recorded.duration_ms(), 500);
        assert_eq!(recorded.peak, 0.5, "the far end never reached the buffer");
    }

    #[tokio::test]
    async fn a_microphone_that_dies_mid_recording_is_an_error_and_not_a_short_clip() {
        // Half a recording is the dangerous outcome: it is long enough to
        // look like an enrolment and short enough to be a worse one than
        // the Operator agreed to. `diarize::enrol` would judge it on its
        // merits and might well accept it, so the refusal has to happen
        // here, where the reason is still known.
        let source = FixtureSource::new(vec![
            Step::audio(AudioChannel::Mic, 300, 0.5),
            Step::fail(AudioChannel::Mic, "the device went away"),
        ]);

        // Matched rather than `expect_err`, which would want `Recorded:
        // Debug` and so a derive that prints half a minute of samples.
        let error = match record(Box::new(source), 1).await {
            Ok(recorded) => panic!(
                "a dead microphone is not a recording: {} samples",
                recorded.samples.len()
            ),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("the device went away"),
            "the reason survives to the Operator: {error}"
        );
    }
}
