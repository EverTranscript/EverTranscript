//! Turning a device buffer into a stamped frame.
//!
//! Both live legs — the microphone through cpal and system audio through a
//! process tap or WASAPI loopback — hand over the same shape of data: an
//! interleaved buffer at whatever rate and channel count the device happens
//! to run at. Both need the same three things done to it, and the third is
//! the one the whole product rests on:
//!
//! 1. downmix to mono, because a leg is one voice-bearing channel;
//! 2. resample to the capture rate, so the two legs share a timebase;
//! 3. **stamp the frame with when its first sample was captured.**
//!
//! Step 3 is why this is shared code rather than two similar loops. Audio
//! position and transcript timing both derive from these stamps, so a leg
//! that computes them slightly differently from the other leg produces a
//! recording where the two halves of a conversation slide apart. Writing the
//! arithmetic once means the legs cannot disagree.

use anyhow::Result;
use evertranscript_protocol::AudioChannel;

use super::dsp::StreamResampler;
use super::AudioFrame;
use super::CaptureClock;
use super::CaptureOffset;
use super::SAMPLE_RATE;

/// Prepares one capture leg's audio for the joiner.
pub struct LegEncoder {
    channel: AudioChannel,
    input_channels: usize,
    input_rate: u32,
    /// Absent when the device already runs at the capture rate.
    resampler: Option<StreamResampler>,
    clock: CaptureClock,
    /// Mono scratch buffer, reused so the callback does not allocate one per
    /// buffer. Audio callbacks run on a deadline; allocation is the classic
    /// way to miss it.
    mono: Vec<f32>,
}

impl LegEncoder {
    pub fn new(
        channel: AudioChannel,
        input_channels: usize,
        input_rate: u32,
        clock: CaptureClock,
    ) -> Result<Self> {
        anyhow::ensure!(
            input_channels > 0,
            "a capture leg needs at least one channel"
        );
        anyhow::ensure!(input_rate > 0, "a capture leg needs a non-zero sample rate");
        let resampler = if input_rate == SAMPLE_RATE {
            None
        } else {
            Some(StreamResampler::new(input_rate, SAMPLE_RATE)?)
        };
        Ok(Self {
            channel,
            input_channels,
            input_rate,
            resampler,
            clock,
            mono: Vec::new(),
        })
    }

    /// Converts one device buffer into a frame, or `None` if it yielded no
    /// audio (an empty buffer, or samples the resampler is still holding).
    pub fn encode(&mut self, interleaved: &[f32]) -> Option<AudioFrame> {
        if interleaved.is_empty() {
            return None;
        }

        // The buffer in hand was captured *before* this call, so the stamp
        // walks back from now: first past whatever the resampler is still
        // holding from earlier buffers, then past this buffer's own span.
        // Reading `now` first keeps that arithmetic against a single instant.
        let now = self.clock.now().millis();
        let held = self.resampler.as_ref().map_or(0, |r| r.pending());
        let frames = interleaved.len() / self.input_channels;
        let span_ms = (frames + held) as u64 * 1000 / self.input_rate as u64;
        let offset = CaptureOffset(now.saturating_sub(span_ms));

        self.mono.clear();
        self.mono.reserve(frames);
        if self.input_channels == 1 {
            self.mono.extend_from_slice(interleaved);
        } else {
            let channels = self.input_channels as f32;
            for frame in interleaved.chunks_exact(self.input_channels) {
                self.mono.push(frame.iter().sum::<f32>() / channels);
            }
        }

        let samples = match self.resampler.as_mut() {
            Some(resampler) => resampler.process(&self.mono),
            None => std::mem::take(&mut self.mono),
        };
        if samples.is_empty() {
            return None;
        }
        Some(AudioFrame::new(self.channel, offset, samples))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stereo_device_at_the_capture_rate_is_downmixed_untouched() {
        let mut encoder = LegEncoder::new(AudioChannel::Mic, 2, SAMPLE_RATE, CaptureClock::start())
            .expect("encoder");
        // Left at 1.0, right at 0.0: the mono average is 0.5.
        let interleaved: Vec<f32> = [1.0f32, 0.0].iter().copied().cycle().take(960).collect();
        let frame = encoder.encode(&interleaved).expect("a frame");
        assert_eq!(frame.samples.len(), 480, "two channels become one");
        assert!(frame.samples.iter().all(|s| (*s - 0.5).abs() < 1e-6));
    }

    #[test]
    fn a_leg_at_another_rate_is_brought_to_the_capture_rate() {
        let mut encoder =
            LegEncoder::new(AudioChannel::System, 1, 44_100, CaptureClock::start()).expect("enc");
        // Feed a second of audio in device-sized buffers.
        let mut produced = 0;
        for _ in 0..100 {
            if let Some(frame) = encoder.encode(&vec![0.25; 441]) {
                produced += frame.samples.len();
            }
        }
        // A second at 44.1 kHz is 48000 samples at the capture rate, less
        // whatever the resampler is still holding.
        assert!(
            (44_000..=48_100).contains(&produced),
            "a second of 44.1 kHz audio should become about a second at 48 kHz, got {produced}"
        );
    }

    #[test]
    fn the_stamp_accounts_for_the_buffer_that_produced_it() {
        // A frame stamped at "now" would place its audio one buffer late,
        // and that error is what desyncs a transcript from its recording.
        let mut encoder = LegEncoder::new(AudioChannel::Mic, 1, SAMPLE_RATE, CaptureClock::start())
            .expect("encoder");
        std::thread::sleep(std::time::Duration::from_millis(120));
        // 100 ms of audio.
        let frame = encoder.encode(&vec![0.1; 4_800]).expect("a frame");
        assert!(
            frame.offset.millis() <= 60,
            "a 100 ms buffer delivered at ~120 ms starts at ~20 ms, got {}",
            frame.offset.millis()
        );
        assert_eq!(frame.duration_ms(), 100);
    }

    #[test]
    fn an_empty_buffer_produces_nothing_rather_than_a_zero_length_frame() {
        let mut encoder = LegEncoder::new(AudioChannel::Mic, 2, SAMPLE_RATE, CaptureClock::start())
            .expect("encoder");
        assert!(encoder.encode(&[]).is_none());
    }

    #[test]
    fn a_nonsense_device_description_is_refused_rather_than_dividing_by_zero() {
        assert!(LegEncoder::new(AudioChannel::Mic, 0, SAMPLE_RATE, CaptureClock::start()).is_err());
        assert!(LegEncoder::new(AudioChannel::Mic, 2, 0, CaptureClock::start()).is_err());
    }
}
