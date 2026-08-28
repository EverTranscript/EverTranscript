//! Simulated acoustic echo, and the number that says whether it was removed.
//!
//! Echo cancellation is impossible to judge by listening in a test, and
//! bit-comparison says nothing. What it needs is a controlled experiment:
//! take far-end audio, put it through a synthetic room, add it to near-end
//! speech, and measure how much of the echo survived.
//!
//! The room here is deliberately simple — a direct path plus a few decaying
//! reflections. It is not a real room, and passing against it is not proof
//! that a speakerphone in a glass meeting room will behave. It is the
//! difference between "the filter converges and removes echo" and "the
//! filter is untested", which is the difference this crate exists for.

/// A synthetic echo path: how the speaker's output reaches the microphone.
#[derive(Debug, Clone)]
pub struct Room {
    /// Direct-path delay. Speaker to microphone across a desk is a few
    /// milliseconds of air plus tens of milliseconds of device buffering.
    pub delay_ms: f32,
    /// How loud the echo is relative to what was played. Real speakerphone
    /// coupling lands well below unity.
    pub gain: f32,
    /// Later reflections, as (extra delay in ms, gain relative to direct).
    pub reflections: Vec<(f32, f32)>,
}

impl Default for Room {
    fn default() -> Self {
        // A laptop on a desk: the speaker is centimetres from the microphone,
        // and most of the delay is the audio stack rather than the air.
        Self {
            delay_ms: 25.0,
            gain: 0.35,
            reflections: vec![(7.0, 0.4), (19.0, 0.22), (37.0, 0.1)],
        }
    }
}

impl Room {
    /// A harder room: more delay and a longer tail.
    pub fn reverberant() -> Self {
        Self {
            delay_ms: 45.0,
            gain: 0.5,
            reflections: vec![(11.0, 0.55), (23.0, 0.4), (41.0, 0.3), (60.0, 0.18)],
        }
    }

    /// The impulse response this room applies, at `rate`.
    pub fn impulse_response(&self, rate: u32) -> Vec<f32> {
        let at = |ms: f32| (ms * rate as f32 / 1000.0).round() as usize;
        let direct = at(self.delay_ms);
        let length = direct
            + self
                .reflections
                .iter()
                .map(|(delay, _)| at(*delay))
                .max()
                .unwrap_or(0)
            + 1;
        let mut response = vec![0.0; length];
        response[direct] = self.gain;
        for (delay, gain) in &self.reflections {
            response[direct + at(*delay)] += self.gain * gain;
        }
        response
    }
}

/// What the microphone hears of audio the speaker played.
///
/// The result is the same length as the input, so it can be added to a
/// near-end signal sample for sample.
pub fn echo_of(far_end: &[f32], rate: u32, room: &Room) -> Vec<f32> {
    let response = room.impulse_response(rate);
    let mut echo = vec![0.0f32; far_end.len()];
    for (tap, gain) in response.iter().enumerate() {
        if *gain == 0.0 {
            continue;
        }
        for index in tap..far_end.len() {
            echo[index] += far_end[index - tap] * gain;
        }
    }
    echo
}

/// Echo Return Loss Enhancement, in decibels: how much quieter the echo got.
///
/// The standard measure for a canceller. Roughly: 0 dB is no cancellation at
/// all, 10 dB is audible improvement, and 20 dB or more is the echo pushed
/// down into the noise. Positive is better.
pub fn erle_db(before: &[f32], after: &[f32]) -> f32 {
    let power =
        |samples: &[f32]| samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32;
    let (before, after) = (power(before), power(after));
    if after <= f32::EPSILON {
        return f32::INFINITY;
    }
    if before <= f32::EPSILON {
        return 0.0;
    }
    10.0 * (before / after).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_room_delays_and_attenuates_what_it_is_given() {
        let room = Room {
            delay_ms: 10.0,
            gain: 0.5,
            reflections: Vec::new(),
        };
        let mut input = vec![0.0f32; 1000];
        input[0] = 1.0;
        let echo = echo_of(&input, 16_000, &room);

        // 10 ms at 16 kHz is 160 samples.
        assert_eq!(echo[160], 0.5);
        assert!(
            echo[..160].iter().all(|s| *s == 0.0),
            "nothing arrives early"
        );
    }

    #[test]
    fn erle_measures_the_reduction_rather_than_the_signal() {
        let loud = vec![0.5f32; 1000];
        let quiet: Vec<f32> = loud.iter().map(|s| s * 0.1).collect();
        // A tenth of the amplitude is a hundredth of the power: 20 dB.
        let erle = erle_db(&loud, &quiet);
        assert!(
            (erle - 20.0).abs() < 0.1,
            "expected about 20 dB, got {erle}"
        );
        assert!(
            erle_db(&loud, &loud).abs() < 0.001,
            "no change is no enhancement"
        );
    }
}
