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
//! see [`super::live::LiveSource::start_microphone_only`].
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
use super::live::LiveSource;

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

/// Records `seconds` of the microphone.
///
/// Drains as it goes rather than reading the channel out afterwards: a
/// half-minute of frames is close enough to the channel's capacity that
/// "the buffer was big enough" would be a thing that quietly stopped being
/// true the day the frame size changed.
pub async fn record(seconds: u64) -> anyhow::Result<Recorded> {
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(4096);
    let mut source = LiveSource::new();
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
