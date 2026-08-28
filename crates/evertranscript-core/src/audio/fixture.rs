//! A scripted capture source.
//!
//! This is the other half of the AudioSource seam: everything downstream of
//! capture — the joiner, the sink, the churn state machine, and later the ASR
//! pipeline — is exercised by feeding it a script instead of a microphone.
//! Device swaps and stream deaths are steps in that script, which is how the
//! ADR-0029 churn contract gets tested without unplugging anything.

use anyhow::Result;
use evertranscript_protocol::AudioChannel;
use tokio::sync::mpsc;

use super::AudioFrame;
use super::AudioSource;
use super::CaptureClock;
use super::CaptureEvent;
use super::CaptureOffset;
use super::SAMPLE_RATE;

/// One scripted event.
#[derive(Debug, Clone)]
pub enum Step {
    /// Audio on one leg: `ms` of a constant sample value.
    Audio {
        channel: AudioChannel,
        ms: u64,
        value: f32,
    },
    /// Audio on one leg from real samples.
    Samples {
        channel: AudioChannel,
        samples: Vec<f32>,
    },
    /// Advance the timeline without producing audio — the shape of a
    /// capture outage.
    Gap { ms: u64 },
    /// The default device changed. Expected housekeeping, not a failure.
    DeviceChange { channel: AudioChannel },
    /// The stream died.
    Fail {
        channel: AudioChannel,
        error: String,
    },
    /// This leg will never produce on this machine.
    Unavailable {
        channel: AudioChannel,
        reason: String,
    },
}

impl Step {
    pub fn audio(channel: AudioChannel, ms: u64, value: f32) -> Self {
        Self::Audio { channel, ms, value }
    }

    pub fn fail(channel: AudioChannel, error: &str) -> Self {
        Self::Fail {
            channel,
            error: error.to_string(),
        }
    }
}

/// Plays a script as fast as the consumer accepts it.
///
/// Offsets come from the script rather than from real elapsed time, so tests
/// are deterministic: a 10-minute meeting runs in milliseconds and always
/// produces the same timeline.
pub struct FixtureSource {
    steps: Vec<Step>,
    /// Per-channel position on the shared clock.
    mic_offset: u64,
    system_offset: u64,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl FixtureSource {
    pub fn new(steps: Vec<Step>) -> Self {
        Self {
            steps,
            mic_offset: 0,
            system_offset: 0,
            handle: None,
        }
    }

    /// A simple two-leg meeting: both channels talking for `ms`.
    pub fn simple(ms: u64) -> Self {
        Self::new(vec![
            Step::audio(AudioChannel::Mic, ms, 0.4),
            Step::audio(AudioChannel::System, ms, -0.4),
        ])
    }

    fn offset_for(&mut self, channel: AudioChannel) -> &mut u64 {
        match channel {
            AudioChannel::Mic => &mut self.mic_offset,
            AudioChannel::System => &mut self.system_offset,
        }
    }

    /// Turns the script into the exact event sequence a live source would
    /// emit, without spawning anything. Useful for direct unit tests.
    pub fn into_events(mut self) -> Vec<CaptureEvent> {
        let steps = std::mem::take(&mut self.steps);
        let mut events = Vec::new();
        for step in steps {
            match step {
                Step::Audio { channel, ms, value } => {
                    let samples = vec![value; (SAMPLE_RATE as u64 * ms / 1000) as usize];
                    let offset = self.offset_for(channel);
                    let start = *offset;
                    *offset += ms;
                    events.push(CaptureEvent::Frame(AudioFrame::new(
                        channel,
                        CaptureOffset(start),
                        samples,
                    )));
                }
                Step::Samples { channel, samples } => {
                    let ms = samples.len() as u64 * 1000 / SAMPLE_RATE as u64;
                    let offset = self.offset_for(channel);
                    let start = *offset;
                    *offset += ms;
                    events.push(CaptureEvent::Frame(AudioFrame::new(
                        channel,
                        CaptureOffset(start),
                        samples,
                    )));
                }
                Step::Gap { ms } => {
                    // Both legs skip forward: nothing was captured, and the
                    // timeline must still account for the time.
                    self.mic_offset += ms;
                    self.system_offset += ms;
                }
                Step::DeviceChange { channel } => {
                    events.push(CaptureEvent::DeviceChanged { channel })
                }
                Step::Fail { channel, error } => {
                    events.push(CaptureEvent::StreamFailed { channel, error })
                }
                Step::Unavailable { channel, reason } => {
                    events.push(CaptureEvent::Unavailable { channel, reason })
                }
            }
        }
        events
    }
}

impl AudioSource for FixtureSource {
    fn start(&mut self, _clock: CaptureClock, events: mpsc::Sender<CaptureEvent>) -> Result<()> {
        let scripted = FixtureSource::new(std::mem::take(&mut self.steps)).into_events();
        self.handle = Some(tokio::spawn(async move {
            for event in scripted {
                if events.send(event).await.is_err() {
                    return;
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_produces_frames_at_the_right_offsets() {
        let events = FixtureSource::new(vec![
            Step::audio(AudioChannel::Mic, 100, 1.0),
            Step::audio(AudioChannel::Mic, 100, 1.0),
        ])
        .into_events();

        let offsets: Vec<u64> = events
            .iter()
            .filter_map(|event| match event {
                CaptureEvent::Frame(frame) => Some(frame.offset.millis()),
                _ => None,
            })
            .collect();
        assert_eq!(offsets, vec![0, 100]);
    }

    #[test]
    fn a_gap_advances_the_clock_without_producing_audio() {
        let events = FixtureSource::new(vec![
            Step::audio(AudioChannel::Mic, 100, 1.0),
            Step::Gap { ms: 200 },
            Step::audio(AudioChannel::Mic, 100, 1.0),
        ])
        .into_events();

        let frames: Vec<&AudioFrame> = events
            .iter()
            .filter_map(|event| match event {
                CaptureEvent::Frame(frame) => Some(frame),
                _ => None,
            })
            .collect();
        assert_eq!(frames.len(), 2, "a gap produces no audio");
        assert_eq!(
            frames[1].offset.millis(),
            300,
            "audio after a gap resumes at real time, not where it left off"
        );
    }

    #[test]
    fn the_two_legs_keep_independent_positions() {
        let events = FixtureSource::new(vec![
            Step::audio(AudioChannel::Mic, 50, 1.0),
            Step::audio(AudioChannel::System, 80, 1.0),
            Step::audio(AudioChannel::Mic, 50, 1.0),
        ])
        .into_events();

        let mic: Vec<u64> = events
            .iter()
            .filter_map(|event| match event {
                CaptureEvent::Frame(frame) if frame.channel == AudioChannel::Mic => {
                    Some(frame.offset.millis())
                }
                _ => None,
            })
            .collect();
        assert_eq!(mic, vec![0, 50], "the mic leg is not shifted by the other");
    }
}
