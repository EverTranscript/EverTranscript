//! 80-mel log filterbank features, in pure Rust.
//!
//! The catalog notes the reference implementation computes these in pure JS,
//! which settles the question a C dependency would otherwise raise: if
//! JavaScript is fast enough, a native FFT crate buys nothing and costs the
//! cross-platform build that ADR-0025's parity gate depends on. M2 ended by
//! finding that an entire platform had never worked while CI was green;
//! adding a C toolchain to the Windows build to save microseconds on a
//! post-meeting job is not a trade this milestone should take.
//!
//! **This is where a diarization pipeline is usually silently wrong.** The
//! model was trained on features computed a particular way — a specific
//! window, a specific mel scale, a specific normalization — and every one of
//! those is invisible if you get it wrong. Nothing crashes. Inference runs.
//! The embeddings are simply meaningless, and the only symptom is that
//! clustering is bad in a way that looks like the model being bad. So each
//! constant below says where it comes from, and the tests assert properties
//! that a wrong implementation could not satisfy by accident.
//!
//! **And it was wrong here, for a milestone.** The first version used a
//! Povey window, a 7.6 kHz top edge, no pre-emphasis, no ×2¹⁵ scaling and
//! no mean normalization. Every one of those is a Kaldi *default* except the
//! last two, and WeSpeaker's recipe — the one this ONNX was trained under,
//! `wespeaker/bin/infer_onnx.py` and pyannote's `compute_fbank` for the same
//! file — is `torchaudio.compliance.kaldi.fbank` with a Hamming window,
//! pre-emphasis 0.97, 20 Hz to Nyquist, waveform ×2¹⁵, then the mean over
//! time subtracted from every bin. Measured on AMI ES2004a (2026-09-15,
//! DECISIONS Q115): embeddings from the old features agreed with the
//! reference's at cosine 0.36 on average, the same speaker in two clean
//! windows scored 0.58 — *below* the 0.62 match floor — and different
//! speakers scored 0.32. With this recipe the agreement is 1.0000, same
//! speaker 0.60, different speakers 0.06. The per-utterance mean is the
//! load-bearing piece; the rest is what closes the last few percent.

/// Sample rate every speaker model here expects.
///
/// Not a preference: the models were trained at 16 kHz, and feeding them
/// 48 kHz audio produces confident nonsense rather than an error.
pub const SAMPLE_RATE: u32 = 16_000;

/// 25 ms window, 10 ms hop — the near-universal speech-features convention
/// these models were trained under.
pub const FRAME_LENGTH: usize = 400;
pub const FRAME_SHIFT: usize = 160;

/// FFT size: the next power of two at or above the frame length.
pub const FFT_SIZE: usize = 512;

/// Mel filters. 80 is what the catalog specifies and what the embedding
/// model expects.
pub const MEL_BINS: usize = 80;

/// The band the filters span: Kaldi's `low-freq` default up to Nyquist,
/// which is what `high-freq = 0` means there and what WeSpeaker trains with.
pub const LOW_FREQ: f32 = 20.0;
pub const HIGH_FREQ: f32 = 8_000.0;

/// Kaldi's pre-emphasis coefficient: `x[n] - 0.97·x[n-1]`, applied after
/// the per-frame DC removal and before the window, as `torchaudio` does.
pub const PREEMPHASIS: f32 = 0.97;

/// WeSpeaker feeds the model features of the waveform scaled to 16-bit
/// range. On its own the scale only shifts every log-mel value by a
/// constant, which the mean normalization below removes again; it is kept
/// so the un-normalized features match the reference to the digit, which
/// is what the golden test `the_features_match_kaldi_to_the_digit` pins.
pub const WAVEFORM_SCALE: f32 = 32_768.0;

/// Hertz to mels, the Slaney/HTK formula the reference uses.
fn hz_to_mel(hz: f32) -> f32 {
    1127.0 * (1.0 + hz / 700.0).ln()
}

#[cfg(test)]
fn mel_to_hz(mel: f32) -> f32 {
    700.0 * ((mel / 1127.0).exp() - 1.0)
}

/// A triangular mel filterbank over the FFT bins.
///
/// Built once and reused: it depends only on constants, and rebuilding it
/// per frame is how feature extraction ends up dominating a pipeline whose
/// actual work is the neural network.
pub struct MelBank {
    /// Per filter: the first FFT bin it touches, and its weights from there.
    filters: Vec<(usize, Vec<f32>)>,
    window: Vec<f32>,
}

impl Default for MelBank {
    fn default() -> Self {
        Self::new()
    }
}

impl MelBank {
    pub fn new() -> Self {
        let bins = FFT_SIZE / 2 + 1;
        let bin_hz = SAMPLE_RATE as f32 / FFT_SIZE as f32;

        let low_mel = hz_to_mel(LOW_FREQ);
        let high_mel = hz_to_mel(HIGH_FREQ);
        // MEL_BINS filters need MEL_BINS + 2 edges: each filter spans three.
        let edges: Vec<f32> = (0..MEL_BINS + 2)
            .map(|index| low_mel + (high_mel - low_mel) * index as f32 / (MEL_BINS + 1) as f32)
            .collect();

        // The ramps are linear in mels, not in hertz: that is how Kaldi
        // builds them, and it moves the low filters' weights by a few
        // percent, which is the difference between features that match
        // the reference to four decimals and to two.
        let mut filters = Vec::with_capacity(MEL_BINS);
        for filter in 0..MEL_BINS {
            let (left, centre, right) = (edges[filter], edges[filter + 1], edges[filter + 2]);
            let mut weights = Vec::new();
            let mut first_bin = None;
            for bin in 0..bins {
                let mel = hz_to_mel(bin as f32 * bin_hz);
                let weight = if mel <= left || mel >= right {
                    0.0
                } else if mel <= centre {
                    (mel - left) / (centre - left)
                } else {
                    (right - mel) / (right - centre)
                };
                if weight > 0.0 {
                    if first_bin.is_none() {
                        first_bin = Some(bin);
                    }
                    weights.push(weight);
                } else if first_bin.is_some() && weights.last().is_some_and(|w| *w > 0.0) {
                    break;
                }
            }
            filters.push((first_bin.unwrap_or(0), weights));
        }

        // Hamming, which WeSpeaker's recipe asks Kaldi for. Not Kaldi's
        // Povey default: that was the first version's mistake, and one of
        // the four the module note counts.
        let window = (0..FRAME_LENGTH)
            .map(|index| {
                let phase = 2.0 * std::f32::consts::PI * index as f32 / (FRAME_LENGTH - 1) as f32;
                0.54 - 0.46 * phase.cos()
            })
            .collect();

        Self { filters, window }
    }

    /// Log-mel features for one channel: `frames × MEL_BINS`, row-major,
    /// with the mean over time removed from every bin — what the embedding
    /// model was trained on, and what makes two windows of one voice at two
    /// levels or on two microphones comparable at all.
    pub fn compute(&self, samples: &[f32]) -> Vec<Vec<f32>> {
        let mut frames = self.raw(samples);
        if frames.is_empty() {
            return frames;
        }
        let mut mean = vec![0.0_f32; MEL_BINS];
        for frame in &frames {
            for (slot, value) in mean.iter_mut().zip(frame) {
                *slot += value;
            }
        }
        for slot in mean.iter_mut() {
            *slot /= frames.len() as f32;
        }
        for frame in frames.iter_mut() {
            for (value, bin_mean) in frame.iter_mut().zip(&mean) {
                *value -= bin_mean;
            }
        }
        frames
    }

    /// The same features before mean normalization: Kaldi's `fbank` output
    /// for a 16-bit-range waveform. Public for the golden test only.
    pub fn raw(&self, samples: &[f32]) -> Vec<Vec<f32>> {
        if samples.len() < FRAME_LENGTH {
            return Vec::new();
        }
        let frame_count = (samples.len() - FRAME_LENGTH) / FRAME_SHIFT + 1;
        let mut frames = Vec::with_capacity(frame_count);

        let mut windowed = vec![0.0_f32; FFT_SIZE];
        let mut frame = vec![0.0_f32; FRAME_LENGTH];
        for index in 0..frame_count {
            let start = index * FRAME_SHIFT;
            for (slot, sample) in frame.iter_mut().zip(&samples[start..start + FRAME_LENGTH]) {
                *slot = sample * WAVEFORM_SCALE;
            }

            // Kaldi's order: remove the frame's DC offset, pre-emphasize,
            // window. A microphone with a DC bias otherwise puts energy in
            // every mel bin and every voice starts to look alike.
            let mean = frame.iter().sum::<f32>() / FRAME_LENGTH as f32;
            for slot in frame.iter_mut() {
                *slot -= mean;
            }
            for position in (1..FRAME_LENGTH).rev() {
                frame[position] -= PREEMPHASIS * frame[position - 1];
            }
            frame[0] -= PREEMPHASIS * frame[0];

            windowed[..FRAME_LENGTH]
                .iter_mut()
                .zip(frame.iter().zip(self.window.iter()))
                .for_each(|(slot, (sample, weight))| *slot = sample * weight);
            windowed[FRAME_LENGTH..].fill(0.0);

            let power = power_spectrum(&windowed);
            let mut mels = Vec::with_capacity(MEL_BINS);
            for (first_bin, weights) in &self.filters {
                let energy: f32 = weights
                    .iter()
                    .enumerate()
                    .map(|(offset, weight)| {
                        power.get(first_bin + offset).copied().unwrap_or(0.0) * weight
                    })
                    .sum();
                // Floored before the log: silence is a real input and
                // ln(0) is not a feature value.
                mels.push(energy.max(1e-10).ln());
            }
            frames.push(mels);
        }
        frames
    }
}

/// Magnitude-squared spectrum via a radix-2 FFT.
///
/// Written out rather than pulled in: the transform is thirty lines and the
/// dependency would be one more thing to cross-compile for the platform this
/// project has already been burned by.
fn power_spectrum(input: &[f32]) -> Vec<f32> {
    let n = FFT_SIZE;
    let mut real: Vec<f32> = input[..n].to_vec();
    let mut imaginary = vec![0.0_f32; n];

    // Bit-reversal permutation.
    let mut target = 0_usize;
    for source in 1..n {
        let mut bit = n >> 1;
        while target & bit != 0 {
            target ^= bit;
            bit >>= 1;
        }
        target |= bit;
        if source < target {
            real.swap(source, target);
            imaginary.swap(source, target);
        }
    }

    let mut length = 2;
    while length <= n {
        let angle = -2.0 * std::f32::consts::PI / length as f32;
        for start in (0..n).step_by(length) {
            for offset in 0..length / 2 {
                let phase = angle * offset as f32;
                let (sin, cos) = phase.sin_cos();
                let a = start + offset;
                let b = a + length / 2;
                let real_b = real[b] * cos - imaginary[b] * sin;
                let imaginary_b = real[b] * sin + imaginary[b] * cos;
                real[b] = real[a] - real_b;
                imaginary[b] = imaginary[a] - imaginary_b;
                real[a] += real_b;
                imaginary[a] += imaginary_b;
            }
        }
        length <<= 1;
    }

    (0..n / 2 + 1)
        .map(|bin| real[bin] * real[bin] + imaginary[bin] * imaginary[bin])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, seconds: f32) -> Vec<f32> {
        let count = (SAMPLE_RATE as f32 * seconds) as usize;
        (0..count)
            .map(|index| {
                (2.0 * std::f32::consts::PI * hz * index as f32 / SAMPLE_RATE as f32).sin()
            })
            .collect()
    }

    #[test]
    fn the_mel_scale_round_trips() {
        for hz in [20.0_f32, 300.0, 1_000.0, 4_000.0, 7_600.0] {
            let back = mel_to_hz(hz_to_mel(hz));
            assert!((back - hz).abs() < 0.1, "{hz} -> {back}");
        }
    }

    #[test]
    fn the_filterbank_covers_the_band_without_gaps() {
        // A gap between filters is silent data loss: a whole frequency range
        // stops reaching the model, and nothing anywhere reports it.
        let bank = MelBank::new();
        assert_eq!(bank.filters.len(), MEL_BINS);
        assert!(
            bank.filters.iter().all(|(_, weights)| !weights.is_empty()),
            "every filter touches at least one FFT bin"
        );
    }

    #[test]
    fn a_pure_tone_lands_in_the_mel_bin_that_contains_it() {
        // The assertion a wrong mel scale, a wrong FFT, or a transposed
        // output cannot satisfy by accident. A 1 kHz tone must peak in the
        // filter whose band contains 1 kHz, and nowhere else.
        let bank = MelBank::new();
        // Raw: the mean over time of a stationary tone *is* the tone, and
        // the normalized features are noise around zero by construction.
        let frames = bank.raw(&tone(1_000.0, 0.2));
        assert!(!frames.is_empty());

        let middle = &frames[frames.len() / 2];
        let peak = middle
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(index, _)| index)
            .expect("a peak");

        // Which filter *should* contain 1 kHz, from the mel spacing.
        let low_mel = hz_to_mel(LOW_FREQ);
        let high_mel = hz_to_mel(HIGH_FREQ);
        let expected = ((hz_to_mel(1_000.0) - low_mel) / (high_mel - low_mel)
            * (MEL_BINS + 1) as f32)
            .round() as usize
            - 1;
        assert!(
            peak.abs_diff(expected) <= 1,
            "1 kHz peaked at filter {peak}, expected around {expected}"
        );
    }

    #[test]
    fn two_different_tones_peak_in_different_places() {
        let bank = MelBank::new();
        let low = bank.raw(&tone(300.0, 0.2));
        let high = bank.raw(&tone(3_000.0, 0.2));

        let peak_of = |frames: &Vec<Vec<f32>>| {
            frames[frames.len() / 2]
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map(|(index, _)| index)
                .expect("a peak")
        };
        assert!(peak_of(&low) < peak_of(&high));
    }

    #[test]
    fn the_frame_count_follows_the_hop() {
        // Off-by-one here shifts every timestamp the model produces, and the
        // symptom is attribution that is subtly early or late everywhere.
        let bank = MelBank::new();
        let one_second = vec![0.0_f32; SAMPLE_RATE as usize];
        let frames = bank.compute(&one_second);
        // (16000 - 400) / 160 + 1
        assert_eq!(frames.len(), 98);
        assert!(frames.iter().all(|frame| frame.len() == MEL_BINS));
    }

    #[test]
    fn silence_produces_finite_features_rather_than_negative_infinity() {
        // Silence is a real and common input — every gap between turns. An
        // unfloored log turns it into -inf, which propagates through the
        // model as NaN and poisons an entire embedding.
        let bank = MelBank::new();
        let frames = bank.compute(&vec![0.0_f32; SAMPLE_RATE as usize]);
        assert!(
            frames.iter().flatten().all(|value| value.is_finite()),
            "silence must be representable"
        );
    }

    #[test]
    fn a_dc_offset_does_not_light_up_every_band() {
        // A microphone with a DC bias is ordinary hardware. Without the
        // per-frame mean removal it puts energy in every mel bin, and every
        // voice starts to look like every other voice.
        let bank = MelBank::new();
        let clean = bank.raw(&tone(1_000.0, 0.2));
        let biased: Vec<f32> = tone(1_000.0, 0.2).iter().map(|s| s + 0.5).collect();
        let offset = bank.raw(&biased);

        let energy = |frames: &Vec<Vec<f32>>| -> f32 {
            frames[frames.len() / 2].iter().sum::<f32>() / MEL_BINS as f32
        };
        assert!(
            (energy(&clean) - energy(&offset)).abs() < 1.0,
            "the bias should be removed, not spread across the bank"
        );
    }

    #[test]
    fn audio_shorter_than_one_frame_produces_nothing_rather_than_panicking() {
        // A 10 ms turn is shorter than the 25 ms window. Real, and it must
        // not index out of bounds.
        let bank = MelBank::new();
        assert!(bank.compute(&[0.0; 160]).is_empty());
        assert!(bank.compute(&[]).is_empty());
    }

    /// A 200→500 Hz chirp under a rising envelope: non-stationary, so every
    /// frame and every bin differs and a wrong window, ramp or pre-emphasis
    /// cannot hide behind symmetry. Reproducible bit-for-bit from the
    /// formula in f32, which is why it is a formula and not a WAV.
    fn chirp() -> Vec<f32> {
        (0..16_000)
            .map(|index| {
                let t = index as f64 / 16_000.0;
                (0.5 * (2.0 * std::f64::consts::PI * (200.0 * t + 150.0 * t * t)).sin()
                    * (0.3 + 0.7 * t)) as f32
            })
            .collect()
    }

    #[test]
    fn the_features_match_kaldi_to_the_digit() {
        // Golden values from `torchaudio.compliance.kaldi.fbank` on the same
        // chirp ×2¹⁵ (num_mel_bins 80, 25/10 ms, hamming, dither 0, no
        // energy) — the call WeSpeaker's `infer_onnx.py` and pyannote's
        // `compute_fbank` make for this exact model. Fails on any of the
        // four mistakes the module note lists; passing is what says the
        // model sees the features it was trained on.
        let frames = MelBank::new().raw(&chirp());
        assert_eq!(frames.len(), 98);
        let golden = [
            (0, 0, 10.1792_f32),
            (0, 5, 19.8904),
            (0, 79, 8.4186),
            (10, 20, 11.1563),
            (25, 40, 9.0054),
            (50, 1, 13.5596),
            (50, 20, 13.6309),
            (50, 70, 10.2246),
            (75, 10, 11.6983),
            (90, 20, 15.4737),
            (97, 40, 13.4940),
            (97, 79, 11.9808),
        ];
        for (frame, bin, expected) in golden {
            let got = frames[frame][bin];
            assert!(
                (got - expected).abs() < 0.01,
                "frame {frame} bin {bin}: got {got}, reference {expected}"
            );
        }
    }

    #[test]
    fn the_mean_over_time_is_removed_from_every_bin() {
        // Per-utterance mean normalization is the piece that was missing
        // and mattered most: without it the same voice twenty decibels
        // apart is two voices. Each bin averages to zero after it.
        let frames = MelBank::new().compute(&chirp());
        for bin in 0..MEL_BINS {
            let mean: f32 =
                frames.iter().map(|frame| frame[bin]).sum::<f32>() / frames.len() as f32;
            assert!(mean.abs() < 1e-3, "bin {bin} has mean {mean}");
        }
        // And it is a shift, not a rescale: a quieter copy of the same
        // audio normalizes to the same features.
        let quiet: Vec<f32> = chirp().iter().map(|s| s * 0.1).collect();
        let quiet_frames = MelBank::new().compute(&quiet);
        for (a, b) in frames[50].iter().zip(&quiet_frames[50]) {
            assert!(
                (a - b).abs() < 1e-3,
                "level changed the features: {a} vs {b}"
            );
        }
    }

    #[test]
    fn the_transform_matches_a_direct_computation() {
        // The FFT is hand-written, so it is checked against the definition
        // rather than trusted. A wrong butterfly produces plausible-looking
        // features that mean nothing.
        let mut input = vec![0.0_f32; FFT_SIZE];
        for (index, slot) in input.iter_mut().enumerate().take(64) {
            *slot = (index as f32 * 0.1).sin();
        }
        let fast = power_spectrum(&input);

        for bin in [0_usize, 1, 7, 64, 200, FFT_SIZE / 2] {
            let mut real = 0.0_f32;
            let mut imaginary = 0.0_f32;
            for (index, sample) in input.iter().enumerate() {
                let phase =
                    -2.0 * std::f32::consts::PI * bin as f32 * index as f32 / FFT_SIZE as f32;
                real += sample * phase.cos();
                imaginary += sample * phase.sin();
            }
            let direct = real * real + imaginary * imaginary;
            let error = (fast[bin] - direct).abs() / direct.max(1e-6);
            assert!(error < 1e-3, "bin {bin}: fft {} vs dft {direct}", fast[bin]);
        }
    }
}
