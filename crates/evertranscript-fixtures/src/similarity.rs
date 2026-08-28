//! Comparing audio by what it sounds like, not by its bytes.
//!
//! Audio that has been resampled, mixed, encoded to AAC and decoded back is
//! never bit-identical to its input. An equality assertion on samples is
//! therefore either permanently red or so loosened it tests nothing. These
//! features — level, brightness, and how the energy is spread across the
//! spectrum — change in ways a human would call "it sounds the same", which
//! is the property tests actually care about.

/// A compact description of what a buffer sounds like.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Features {
    /// Loudness.
    pub rms: f32,
    /// Highest absolute sample — catches clipping and gain mistakes.
    pub peak: f32,
    /// Sign changes per sample: a cheap brightness/noisiness proxy.
    pub zero_crossing_rate: f32,
    /// Spectral centre of mass, in Hz. Roughly "brightness".
    pub spectral_centroid: f32,
    /// Energy fractions in low (<300 Hz), mid (300–3400 Hz), and high
    /// (>3400 Hz) bands. The mid band is the speech band.
    pub band_energy: [f32; 3],
}

impl Features {
    pub fn of(samples: &[f32], rate: u32) -> Self {
        if samples.is_empty() {
            return Self {
                rms: 0.0,
                peak: 0.0,
                zero_crossing_rate: 0.0,
                spectral_centroid: 0.0,
                band_energy: [0.0; 3],
            };
        }

        let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
        let peak = samples.iter().fold(0.0f32, |max, s| max.max(s.abs()));
        let crossings = samples
            .windows(2)
            .filter(|pair| (pair[0] < 0.0) != (pair[1] < 0.0))
            .count();
        let zero_crossing_rate = crossings as f32 / samples.len() as f32;

        let spectrum = average_spectrum(samples);
        let bin_hz = rate as f32 / (spectrum.len() * 2) as f32;

        let total: f32 = spectrum.iter().sum();
        let (mut weighted, mut bands) = (0.0f32, [0.0f32; 3]);
        for (index, magnitude) in spectrum.iter().enumerate() {
            let frequency = index as f32 * bin_hz;
            weighted += frequency * magnitude;
            let band = if frequency < 300.0 {
                0
            } else if frequency <= 3400.0 {
                1
            } else {
                2
            };
            bands[band] += magnitude;
        }
        let spectral_centroid = if total > 0.0 { weighted / total } else { 0.0 };
        if total > 0.0 {
            for band in &mut bands {
                *band /= total;
            }
        }

        Self {
            rms,
            peak,
            zero_crossing_rate,
            spectral_centroid,
            band_energy: bands,
        }
    }

    /// Fails with a readable message when two buffers do not sound alike.
    ///
    /// `tolerance` is a fraction: 0.15 means each feature may differ by 15%.
    pub fn assert_similar(&self, other: &Features, tolerance: f32, context: &str) {
        let checks: [(&str, f32, f32); 4] = [
            ("rms", self.rms, other.rms),
            ("peak", self.peak, other.peak),
            (
                "zero-crossing rate",
                self.zero_crossing_rate,
                other.zero_crossing_rate,
            ),
            (
                "spectral centroid",
                self.spectral_centroid,
                other.spectral_centroid,
            ),
        ];
        for (name, expected, actual) in checks {
            let scale = expected.abs().max(actual.abs()).max(1e-6);
            let difference = (expected - actual).abs() / scale;
            assert!(
                difference <= tolerance,
                "{context}: {name} differs by {:.1}% (expected {expected:.5}, got {actual:.5}); \
                 tolerance is {:.0}%",
                difference * 100.0,
                tolerance * 100.0
            );
        }
        for (index, label) in ["low", "mid (speech)", "high"].iter().enumerate() {
            let difference = (self.band_energy[index] - other.band_energy[index]).abs();
            assert!(
                difference <= tolerance,
                "{context}: {label}-band energy differs by {:.3} (expected {:.3}, got {:.3})",
                difference,
                self.band_energy[index],
                other.band_energy[index]
            );
        }
    }
}

/// How well one buffer preserves another's level, as a percentage.
///
/// A resampler that returns 173% here is the exact bug Meetily shipped by
/// constructing a resampler per chunk instead of keeping one. Ticket 08
/// asserts on this.
pub fn rms_preservation_percent(original: &[f32], processed: &[f32]) -> f32 {
    let before = Features::of(original, 16_000).rms;
    let after = Features::of(processed, 16_000).rms;
    if before <= f32::EPSILON {
        return 100.0;
    }
    after / before * 100.0
}

/// Averaged magnitude spectrum over overlapping windows.
fn average_spectrum(samples: &[f32]) -> Vec<f32> {
    const WINDOW: usize = 1024;
    if samples.len() < WINDOW {
        return magnitude_spectrum(&padded(samples, WINDOW));
    }
    let hop = WINDOW / 2;
    let mut accumulated = vec![0.0f32; WINDOW / 2];
    let mut windows = 0;
    for start in (0..=samples.len() - WINDOW).step_by(hop) {
        let spectrum = magnitude_spectrum(&samples[start..start + WINDOW]);
        for (slot, value) in accumulated.iter_mut().zip(spectrum) {
            *slot += value;
        }
        windows += 1;
    }
    if windows > 0 {
        for slot in &mut accumulated {
            *slot /= windows as f32;
        }
    }
    accumulated
}

fn padded(samples: &[f32], length: usize) -> Vec<f32> {
    let mut out = samples.to_vec();
    out.resize(length, 0.0);
    out
}

/// Magnitude spectrum of one Hann-windowed frame (radix-2, in place).
fn magnitude_spectrum(frame: &[f32]) -> Vec<f32> {
    let n = frame.len();
    debug_assert!(n.is_power_of_two(), "the FFT needs a power-of-two window");

    let mut real: Vec<f32> = frame
        .iter()
        .enumerate()
        .map(|(index, sample)| {
            // Hann window: without it, the frame edges act like impulses and
            // smear energy across the whole spectrum.
            let window = 0.5 - 0.5 * (std::f32::consts::TAU * index as f32 / (n - 1) as f32).cos();
            sample * window
        })
        .collect();
    let mut imaginary = vec![0.0f32; n];

    // Bit-reversal permutation.
    let mut target = 0usize;
    for source in 0..n {
        if source < target {
            real.swap(source, target);
            imaginary.swap(source, target);
        }
        let mut mask = n >> 1;
        while mask > 0 && target & mask != 0 {
            target &= !mask;
            mask >>= 1;
        }
        target |= mask;
    }

    // Danielson–Lanczos butterflies.
    let mut length = 2;
    while length <= n {
        let angle = -std::f32::consts::TAU / length as f32;
        for start in (0..n).step_by(length) {
            for offset in 0..length / 2 {
                let theta = angle * offset as f32;
                let (sin, cos) = theta.sin_cos();
                let index = start + offset;
                let pair = index + length / 2;
                let real_part = cos * real[pair] - sin * imaginary[pair];
                let imaginary_part = sin * real[pair] + cos * imaginary[pair];
                real[pair] = real[index] - real_part;
                imaginary[pair] = imaginary[index] - imaginary_part;
                real[index] += real_part;
                imaginary[index] += imaginary_part;
            }
        }
        length <<= 1;
    }

    (0..n / 2)
        .map(|index| (real[index] * real[index] + imaginary[index] * imaginary[index]).sqrt())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frequency: f32, seconds: f32, rate: u32) -> Vec<f32> {
        let count = (seconds * rate as f32) as usize;
        (0..count)
            .map(|index| {
                (index as f32 / rate as f32 * frequency * std::f32::consts::TAU).sin() * 0.5
            })
            .collect()
    }

    #[test]
    fn the_spectrum_finds_the_tone_it_is_given() {
        let features = Features::of(&sine(1000.0, 0.5, 16_000), 16_000);
        assert!(
            (features.spectral_centroid - 1000.0).abs() < 120.0,
            "a 1 kHz tone should centre near 1 kHz, got {}",
            features.spectral_centroid
        );
        assert!(
            features.band_energy[1] > 0.7,
            "1 kHz belongs to the speech band, got {:?}",
            features.band_energy
        );
    }

    #[test]
    fn brightness_separates_a_low_tone_from_a_high_one() {
        let low = Features::of(&sine(200.0, 0.5, 16_000), 16_000);
        let high = Features::of(&sine(5000.0, 0.5, 16_000), 16_000);
        assert!(low.spectral_centroid < high.spectral_centroid);
        assert!(low.zero_crossing_rate < high.zero_crossing_rate);
        assert!(low.band_energy[0] > 0.5, "200 Hz is a low-band tone");
        assert!(high.band_energy[2] > 0.5, "5 kHz is a high-band tone");
    }

    #[test]
    fn similarity_accepts_a_small_gain_change_and_rejects_a_large_one() {
        let original = sine(440.0, 0.3, 16_000);
        let slightly_quieter: Vec<f32> = original.iter().map(|s| s * 0.95).collect();
        let halved: Vec<f32> = original.iter().map(|s| s * 0.5).collect();

        let reference = Features::of(&original, 16_000);
        reference.assert_similar(
            &Features::of(&slightly_quieter, 16_000),
            0.15,
            "a 5% gain change",
        );

        let result = std::panic::catch_unwind(|| {
            reference.assert_similar(&Features::of(&halved, 16_000), 0.15, "halved");
        });
        assert!(result.is_err(), "halving the level must fail the check");
    }

    #[test]
    fn rms_preservation_catches_the_amplifying_resampler_bug() {
        let original = sine(440.0, 0.2, 16_000);
        let correct: Vec<f32> = original.clone();
        let amplifying: Vec<f32> = original.iter().map(|s| s * 1.735).collect();

        assert!((rms_preservation_percent(&original, &correct) - 100.0).abs() < 0.1);
        let broken = rms_preservation_percent(&original, &amplifying);
        assert!(
            broken > 170.0,
            "the 173%-RMS bug class must be visible, got {broken}"
        );
    }

    #[test]
    fn empty_audio_has_defined_features() {
        let features = Features::of(&[], 16_000);
        assert_eq!(features.rms, 0.0);
        assert_eq!(features.spectral_centroid, 0.0);
    }
}
