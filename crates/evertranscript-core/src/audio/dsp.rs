//! Signal conditioning on the microphone leg.
//!
//! Two jobs, both aimed at the same failure: the quiet talker. A far-end
//! participant on a bad connection, or an Operator who leans back from the
//! laptop, produces audio that a transcription model hears as noise. Level
//! is the cheapest quality lever there is.
//!
//! **Echo cancellation is not here yet.** ADR-0029 requires it — on
//! speakers, the far end re-enters the microphone, breaking both the channel
//! attribution and the transcript. It needs the DTLN ONNX models and an
//! inference runtime, neither of which this build ships. The gap is real and
//! is recorded in the ticket rather than hidden behind a stub that does
//! nothing.

use ebur128::EbuR128;
use rubato::Resampler;
use rubato::SincFixedIn;
use rubato::SincInterpolationParameters;
use rubato::SincInterpolationType;
use rubato::WindowFunction;
use tracing::debug;

/// EBU R128 programme loudness target. −23 LUFS is the broadcast standard
/// and what the shipped notetakers normalize to.
pub const TARGET_LUFS: f64 = -23.0;

/// Never amplify past this. Beyond it, a quiet passage is being asked to
/// carry information that is not in it, and the result is amplified room
/// tone that a model then tries to transcribe.
const MAX_GAIN: f32 = 8.0;

/// Below this, a channel is treated as holding nothing worth lifting.
///
/// Amplifying a channel with no speech in it amplifies whatever noise it does
/// have, and the consequence is not merely ugly: whisper *invents* words on
/// near-silence. A cancelled microphone channel came back as "Það er hér" and
/// "Gracias" — confident, fabricated, and attributed to the Operator, because
/// the gain below had lifted its residual past the speech gate.
///
/// The guard used to be −70 LUFS, which sits under everything and so caught
/// nothing. Measured on this machine's recordings: real speech runs −17 to
/// −20 LUFS, while an echo-cancelled microphone channel runs −55 to −67. The
/// gap is 35 dB, so this sits in the middle with room on both sides — a
/// genuinely quiet talker is still lifted, and a channel that is only room
/// tone is left where it is.
const NOTHING_TO_LIFT_LUFS: f64 = -50.0;

/// True-peak ceiling. Leaves headroom so the AAC encoder never clips.
const PEAK_CEILING: f32 = 0.891; // −1 dBFS

/// Brings a leg toward the loudness target without letting it clip.
///
/// Deliberately slow-moving: gain that chases every syllable is compression,
/// which changes how speech sounds and makes speaker attribution harder. One
/// gain per meeting, adjusted gradually, is what a level control should do.
pub struct LoudnessNormalizer {
    meter: EbuR128,
    gain: f32,
    /// Samples measured so far; loudness is meaningless below a few seconds.
    measured: usize,
    rate: u32,
}

impl LoudnessNormalizer {
    pub fn new(rate: u32) -> anyhow::Result<Self> {
        let meter = EbuR128::new(1, rate, ebur128::Mode::I | ebur128::Mode::TRUE_PEAK)
            .map_err(|error| anyhow::anyhow!("creating the loudness meter: {error:?}"))?;
        Ok(Self {
            meter,
            gain: 1.0,
            measured: 0,
            rate,
        })
    }

    /// The gain currently being applied.
    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// Normalizes in place and updates the running measurement.
    pub fn process(&mut self, samples: &mut [f32]) {
        if samples.is_empty() {
            return;
        }
        if self.meter.add_frames_f32(samples).is_ok() {
            self.measured += samples.len();
        }

        // Three seconds is the shortest window where integrated loudness
        // means anything; before that, leave the audio alone.
        let enough = self.measured >= self.rate as usize * 3;
        if enough
            && let Ok(loudness) = self.meter.loudness_global()
            && loudness.is_finite()
        {
            // A channel with nothing in it is returned to unity rather than
            // left holding whatever gain it earned while it did have
            // something — otherwise a leg that goes quiet keeps its boost and
            // amplifies the room.
            let wanted = if loudness > NOTHING_TO_LIFT_LUFS {
                (10f64.powf((TARGET_LUFS - loudness) / 20.0) as f32)
                    .clamp(1.0 / MAX_GAIN, MAX_GAIN)
            } else {
                1.0
            };
            // Move a little at a time: a jump would be audible and
            // would change the level mid-sentence.
            self.gain += (wanted - self.gain) * 0.05;
        }

        for sample in samples.iter_mut() {
            *sample = (*sample * self.gain).clamp(-PEAK_CEILING, PEAK_CEILING);
        }
    }
}

/// Sample-rate conversion that keeps its state across buffers.
///
/// One resampler for the life of a stream, fed fixed-size chunks. Building a
/// resampler per chunk is a real and subtle bug — it discards the filter
/// state at every boundary, which Meetily shipped as audio arriving at 173%
/// of its original level. `rms_preservation_percent` in the fixtures crate
/// exists to catch exactly that, and the test below uses it.
pub struct StreamResampler {
    inner: SincFixedIn<f32>,
    /// Input samples not yet consumed: the resampler takes fixed-size
    /// chunks, and callers do not deliver them that way.
    pending: Vec<f32>,
    chunk: usize,
}

impl StreamResampler {
    pub fn new(from_rate: u32, to_rate: u32) -> anyhow::Result<Self> {
        // 256 taps with linear interpolation is the quality/cost point for
        // downsampling speech; the ratio here is a simple 3:1.
        let parameters = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        };
        let chunk = 1024;
        let inner =
            SincFixedIn::<f32>::new(to_rate as f64 / from_rate as f64, 2.0, parameters, chunk, 1)
                .map_err(|error| anyhow::anyhow!("creating the resampler: {error}"))?;
        Ok(Self {
            inner,
            pending: Vec::new(),
            chunk,
        })
    }

    /// Input samples held back from earlier calls.
    ///
    /// Capture uses this to stamp a frame: the output covers audio starting
    /// this many samples *before* the buffer just handed in, and a leg that
    /// ignored the backlog would drift later by up to one chunk.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Resamples what it can, holding the remainder for the next call so no
    /// sample is dropped at a buffer boundary.
    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        self.pending.extend_from_slice(input);
        let mut out = Vec::new();
        while self.pending.len() >= self.chunk {
            let block: Vec<f32> = self.pending.drain(..self.chunk).collect();
            match self.inner.process(&[block], None) {
                Ok(resampled) => {
                    if let Some(channel) = resampled.into_iter().next() {
                        out.extend(channel);
                    }
                }
                Err(error) => {
                    debug!(%error, "resampling failed; dropping this block");
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(frequency: f32, seconds: f32, rate: u32, amplitude: f32) -> Vec<f32> {
        let count = (seconds * rate as f32) as usize;
        (0..count)
            .map(|index| {
                (index as f32 / rate as f32 * frequency * std::f32::consts::TAU).sin() * amplitude
            })
            .collect()
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt()
    }

    #[test]
    fn a_quiet_talker_is_brought_up() {
        let mut normalizer = LoudnessNormalizer::new(16_000).expect("normalizer");
        let quiet = tone(200.0, 10.0, 16_000, 0.02);
        let before = rms(&quiet);

        // Process in realistic chunks so the meter accumulates as it would
        // during a meeting.
        let mut processed = Vec::new();
        for block in quiet.chunks(1600) {
            let mut block = block.to_vec();
            normalizer.process(&mut block);
            processed.extend(block);
        }

        // The tail is what matters: the gain ramps in rather than jumping.
        let tail = &processed[processed.len() / 2..];
        assert!(
            rms(tail) > before * 1.5,
            "quiet speech should be lifted (before {before:.4}, after {:.4})",
            rms(tail)
        );
        assert!(normalizer.gain() > 1.0, "gain should have risen");
    }

    #[test]
    fn loud_audio_is_never_amplified_into_clipping() {
        let mut normalizer = LoudnessNormalizer::new(16_000).expect("normalizer");
        let loud = tone(200.0, 10.0, 16_000, 0.9);

        let mut processed = Vec::new();
        for block in loud.chunks(1600) {
            let mut block = block.to_vec();
            normalizer.process(&mut block);
            processed.extend(block);
        }

        let peak = processed.iter().fold(0.0f32, |max, s| max.max(s.abs()));
        assert!(
            peak <= PEAK_CEILING + 1e-4,
            "output must stay under the peak ceiling, got {peak}"
        );
    }

    #[test]
    fn a_channel_with_only_residual_in_it_is_not_lifted_into_speech() {
        // `silence_is_left_alone` below tests *digital* zero, which no real
        // channel ever is. The case that matters is a microphone leg after
        // echo cancellation: not silent, just empty — and lifting it eight
        // times put its residual above the speech gate, where whisper
        // invented "Það er hér" and "Gracias" and attributed them to the
        // Operator.
        //
        // −60 dBFS of noise is what that leg measures. Real speech runs 35 dB
        // above it.
        let mut normalizer = LoudnessNormalizer::new(16_000).expect("normalizer");
        let mut residual: Vec<f32> = (0..16_000 * 5)
            .map(|i| ((i as f32 * 0.37).sin() + (i as f32 * 1.31).sin()) * 0.0005)
            .collect();
        let before = rms(&residual);
        normalizer.process(&mut residual);
        let after = rms(&residual);

        assert!(
            after <= before * 1.05,
            "a channel with nothing in it must not be amplified: {before:.6} \
             became {after:.6} (gain {:.2})",
            normalizer.gain()
        );
    }

    #[test]
    fn silence_is_left_alone() {
        // Amplifying silence produces amplified room tone, which is
        // hallucination fuel.
        let mut normalizer = LoudnessNormalizer::new(16_000).expect("normalizer");
        let mut silence = vec![0.0f32; 16_000 * 5];
        normalizer.process(&mut silence);
        assert!(silence.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn the_resampler_preserves_level_across_buffer_boundaries() {
        // The 173%-amplitude bug: a resampler rebuilt per chunk loses its
        // filter state at every boundary. This feeds deliberately ragged
        // buffer sizes, which is where that shows up.
        let mut resampler = StreamResampler::new(48_000, 16_000).expect("resampler");
        let input = tone(440.0, 2.0, 48_000, 0.5);

        let mut output = Vec::new();
        let mut offset = 0;
        for size in [1000, 777, 2048, 333, 4096].iter().cycle() {
            if offset >= input.len() {
                break;
            }
            let end = (offset + size).min(input.len());
            output.extend(resampler.process(&input[offset..end]));
            offset = end;
        }

        let preservation =
            evertranscript_fixtures::similarity::rms_preservation_percent(&input, &output);
        assert!(
            (97.0..=103.0).contains(&preservation),
            "level must survive resampling, got {preservation:.1}%"
        );
    }

    #[test]
    fn the_resampler_produces_roughly_the_right_number_of_samples() {
        let mut resampler = StreamResampler::new(48_000, 16_000).expect("resampler");
        let input = tone(440.0, 1.0, 48_000, 0.5);
        let output = resampler.process(&input);

        // Within one chunk's worth: the tail is held for the next call.
        assert!(
            (output.len() as i64 - 16_000).abs() < 1200,
            "one second at 48 kHz should become about 16000 samples, got {}",
            output.len()
        );
    }
}
