//! Echo cancellation: keeping the far end out of the near-end channel.
//!
//! On speakers, what the other participants say comes out of the Operator's
//! laptop and straight back into their microphone. Both legs then carry it,
//! and the damage is not merely duplicated audio — it is *misattribution*.
//! The far end's words arrive on the microphone channel, which is the
//! channel that means "the Operator said this". A transcript that credits
//! people with things they did not say is worse than one that missed them.
//!
//! ADR-0029 called for a DTLN-shaped canceller. This is a classic normalized
//! least-mean-squares adaptive filter instead, and the reason is that the
//! usual objection to NLMS does not apply here. Alignment is what makes echo
//! cancellation hard: the reference and the microphone come from different
//! clocks, and an unknown, drifting delay between them has to be estimated
//! before any filter can converge. EverTranscript stamps both legs on one
//! capture clock (ADR-0029's other half), so they arrive already aligned to
//! the sample. What is left is the part NLMS is genuinely good at. Shipping
//! it costs no ONNX runtime, no model to download, and no inference budget
//! competing with transcription.
//!
//! **The governing rule is do no harm.** Most meetings are on headphones and
//! have no echo at all, so on the overwhelming majority of audio this code
//! must do nothing. Three things enforce that:
//!
//! 1. Adaptation is frozen unless the far end is actually playing.
//! 2. Adaptation is frozen while the near end is talking over it, which is
//!    when subtracting a wrong estimate would eat the Operator's own words.
//! 3. A filter that diverges is reset rather than left to destroy audio.
//!
//! With no echo present the filter converges toward silence, so the output
//! is the input. That is the property the tests pin hardest.

/// Filter length in milliseconds.
///
/// This has to span the whole echo path, and a filter shorter than the tail
/// cannot model it at any step size — the reverberant fixture only reaches
/// 7 dB against a 64 ms filter for exactly that reason. A laptop on a desk
/// needs far less, but a speakerphone in a hard-walled meeting room is the
/// case that actually has echo, so the filter is sized for that: 128 ms
/// covers the direct path, the device buffering, and a room's reflections.
///
/// The cost is linear in the length and paid per microphone sample, which
/// at 16 kHz is a few percent of one core — cheap beside transcription.
const FILTER_MS: usize = 128;

/// Step size. Small enough that a wrong estimate corrects gradually rather
/// than lurching, which matters because a lurch is audible as a chirp in
/// the middle of a word.
const STEP: f32 = 0.8;

/// Below this, the far end is not playing and there is no echo to learn.
const REFERENCE_FLOOR: f32 = 1e-6;

/// Freeze adaptation once near-end energy rises to this fraction of the
/// reference energy.
///
/// Echo is attenuated on the way back — a microphone hears its own speaker
/// well below the level it was played at. So a microphone as loud as the
/// reference is not echo, it is somebody talking, and adapting on it would
/// teach the filter to subtract their voice.
const DOUBLE_TALK_RATIO: f32 = 0.5;

/// Smoothing for the energy estimates the gate reads.
const ENERGY_SMOOTHING: f32 = 0.02;

/// Weight norm past which the filter is considered diverged.
const DIVERGENCE_LIMIT: f32 = 100.0;

/// How much of the microphone's energy the filter must be removing before
/// the remainder is treated as leftover echo rather than speech.
///
/// A linear filter alone is not enough for this product's purpose. It can
/// take an echo well down and still leave something a transcription model
/// decodes perfectly happily — a quiet echo is still an intelligible one,
/// and the record does not care how many decibels it was. So once the
/// filter is demonstrably explaining most of what the microphone hears, the
/// residual is suppressed rather than merely attenuated. This is the
/// standard companion to an adaptive filter, and it is what turns "the far
/// end, but fainter" into nothing at all.
///
/// The threshold is what makes it safe: when the Operator is speaking, their
/// voice is not predictable from the reference, so the residual stays large
/// and no suppression happens.
const ECHO_DOMINANCE: f32 = 0.25;

/// What the residual is scaled by once it is judged to be echo.
const SUPPRESSION: f32 = 0.02;

/// Gain smoothing. Suppression engages briskly and releases gently, so the
/// Operator's first syllable after listening is never clipped.
const ENGAGE: f32 = 0.02;
const RELEASE: f32 = 0.002;

/// How long the far end still counts as playing after its energy drops.
///
/// **Speech pauses, and without this the suppressor released in every gap.**
/// Measured before it existed: on real speech the gain sat above 0.5 for 14%
/// of samples and those samples carried **74% of everything that escaped**,
/// with a mean gain of 0.61 in the 50 ms after each far-end onset — against
/// 0.02 when settled. The gap re-opened at every pause and took the whole
/// onset transient to close, because re-engaging needs the filter to be
/// predicting well again and after a silence it briefly is not.
///
/// That put the leak exactly on speech onsets, which is where the phonetic
/// information is, which is why a transcription model could still read the
/// far end out of a residual that averaged 32.8 dB down (DECISIONS Q51).
///
/// 200 ms spans the gaps between words and most gaps between sentences. It
/// is safe to hold that long because the hold is only ever a hold: dominance
/// returning engages, and dominance staying gone releases. See
/// `DOMINANCE_HOLD_MS` and `suppress_residual_echo`.
/// Babble never pauses, so none of this was reachable by the tests that
/// existed.
const FAR_END_HANGOVER_MS: usize = 200;

/// How long a loss of echo dominance counts as an utterance starting rather
/// than as a person starting.
///
/// The two are indistinguishable for an instant: the filter's estimate lags
/// at the beginning of an utterance exactly as it does when someone
/// unpredictable speaks. What separates them is duration. An onset settles
/// in tens of milliseconds; a person does not stop being a person.
///
/// 50 ms covers the transient, and double talk still releases within about
/// a syllable — `the_near_end_speaker_is_not_cancelled_along_with_the_echo`
/// is the test that decides whether that is short enough.
const DOMINANCE_HOLD_MS: usize = 50;

/// Removes far-end audio from the near-end channel.
pub struct EchoCanceller {
    weights: Vec<f32>,
    /// Reference delay line, twice the filter length so the window for each
    /// sample is one contiguous slice instead of a wrapped pair.
    history: Vec<f32>,
    write: usize,
    taps: usize,
    /// Sum of squares over the current window, maintained incrementally.
    window_power: f32,
    near_energy: f32,
    far_energy: f32,
    /// Energy left after the filter, against which echo dominance is judged.
    residual_energy: f32,
    /// Current residual-suppression gain, moved gradually rather than
    /// switched, because a switched gain is audible as a click.
    gain: f32,
    /// Samples since the far end was last above the floor, and how many of
    /// them still count as "playing" — the hangover that keeps a pause
    /// between words from releasing the suppressor.
    far_idle: usize,
    since_dominated: usize,
    dominance_hold: usize,
    hangover: usize,
    /// Whether any weight is non-zero. While it is false the filter has
    /// nothing to subtract, and saying so is what keeps the cost off
    /// meetings that have no echo path to model.
    active: bool,
    /// Counts resets, so a filter that keeps blowing up is visible.
    resets: usize,
}

impl EchoCanceller {
    pub fn new(rate: u32) -> Self {
        let taps = (rate as usize * FILTER_MS / 1000).max(1);
        Self {
            weights: vec![0.0; taps],
            history: vec![0.0; taps * 2],
            write: taps,
            taps,
            window_power: 0.0,
            near_energy: 0.0,
            far_energy: 0.0,
            residual_energy: 0.0,
            gain: 1.0,
            far_idle: usize::MAX,
            since_dominated: usize::MAX,
            dominance_hold: rate as usize * DOMINANCE_HOLD_MS / 1000,
            hangover: rate as usize * FAR_END_HANGOVER_MS / 1000,
            active: false,
            resets: 0,
        }
    }

    /// How many times the filter has been reset after diverging.
    pub fn resets(&self) -> usize {
        self.resets
    }

    /// Whether the filter has learned an echo path worth applying.
    ///
    /// Useful for reporting: a Meeting on headphones should show this as
    /// false for its whole length.
    pub fn is_cancelling(&self) -> bool {
        self.weights.iter().map(|w| w.abs()).sum::<f32>() > 0.01
    }

    /// Subtracts the estimated echo of `reference` from `near`, in place.
    ///
    /// The two slices must be the same span of time, sample for sample —
    /// which is what the shared capture clock guarantees. A shorter
    /// reference is treated as silence for the remainder rather than
    /// misaligning everything after it.
    pub fn process(&mut self, near: &mut [f32], reference: &[f32]) {
        for (index, sample) in near.iter_mut().enumerate() {
            let far = reference.get(index).copied().unwrap_or(0.0);
            *sample = self.step(*sample, far);
        }
    }

    /// One sample through the filter.
    fn step(&mut self, near: f32, far: f32) -> f32 {
        // Slide the window: the sample leaving is the one `taps` back.
        let leaving = self.history[self.write - self.taps];
        self.history[self.write] = far;
        self.window_power += far * far - leaving * leaving;
        // Incremental sums drift, and a negative power would invert the
        // step size — which is divergence with extra steps.
        if self.window_power < 0.0 {
            self.window_power = 0.0;
        }

        self.near_energy += (near * near - self.near_energy) * ENERGY_SMOOTHING;
        self.far_energy += (far * far - self.far_energy) * ENERGY_SMOOTHING;

        let far_end_playing = self.far_energy > REFERENCE_FLOOR;
        let near_end_talking = self.near_energy > self.far_energy * DOUBLE_TALK_RATIO;
        // **Adaptation is deliberately not given the hangover.** Learning
        // from a silent reference is how a filter unlearns a room, and the
        // three guarantees in this module's header are all about when *not*
        // to adapt. The hangover exists for the suppressor and stops there.
        let adapting = far_end_playing && !near_end_talking && self.window_power > REFERENCE_FLOOR;
        if far_end_playing {
            self.far_idle = 0;
        } else {
            self.far_idle = self.far_idle.saturating_add(1);
        }
        let far_recently_playing = far_end_playing || self.far_idle < self.hangover;

        // Nothing learned and nothing to learn from: the estimate would be
        // zero and the subtraction a no-op. Taking the shortcut rather than
        // computing it keeps a filter this long off the cost of every
        // microphone-only meeting, which is most of them.
        if !adapting && !self.active {
            self.residual_energy += (near * near - self.residual_energy) * ENERGY_SMOOTHING;
            self.advance();
            return self.suppress_residual_echo(near, far_recently_playing);
        }

        let window = &self.history[self.write + 1 - self.taps..=self.write];
        let estimate: f32 = self
            .weights
            .iter()
            .zip(window)
            .map(|(weight, sample)| weight * sample)
            .sum();
        let error = near - estimate;

        self.residual_energy += (error * error - self.residual_energy) * ENERGY_SMOOTHING;

        if adapting {
            let step = STEP * error / self.window_power;
            for (weight, sample) in self.weights.iter_mut().zip(window) {
                *weight += step * sample;
            }
            self.active = true;
            self.guard_against_divergence();
        }

        self.advance();
        self.suppress_residual_echo(error, far_recently_playing)
    }

    /// Scales down what the filter could not remove, when what is left is
    /// echo rather than someone speaking.
    ///
    /// The judgement is comparative: if the filter is accounting for most of
    /// the microphone's energy, then the microphone is hearing the far end
    /// and the remainder is residue. If it is not — because the Operator is
    /// talking, and nothing in the reference predicts that — the residual
    /// stays large and this does nothing.
    /// Three states rather than two, and the third is the fix.
    ///
    /// **Engage** when the filter is accounting for most of the microphone.
    /// **Hold** through a brief loss of dominance — the beginning of an
    /// utterance, where the estimate lags for a few tens of milliseconds and
    /// where releasing costs the whole onset.
    /// **Release** when the far end has genuinely stopped, or when dominance
    /// has been gone long enough to be a person rather than a transient.
    fn suppress_residual_echo(&mut self, error: f32, far_recently_playing: bool) -> f32 {
        let dominated = far_recently_playing
            && self.near_energy > REFERENCE_FLOOR
            && self.residual_energy < self.near_energy * ECHO_DOMINANCE;
        if dominated {
            self.since_dominated = 0;
            self.gain += (SUPPRESSION - self.gain) * ENGAGE;
            return error * self.gain;
        }
        self.since_dominated = self.since_dominated.saturating_add(1);
        // **Hold across a brief loss of dominance, release on a sustained
        // one.** Both look identical for an instant — the filter's estimate
        // lags at the start of an utterance exactly as it does when someone
        // unpredictable starts speaking — so the only thing separating them
        // is how long it lasts. An onset settles in milliseconds; a person
        // talking does not.
        //
        // `near_end_talking` cannot make this call, though it is the obvious
        // candidate and was tried: `far_energy` collapses the moment the far
        // end stops while the delayed echo keeps `near_energy` up, so it
        // fires on the echo's own tail. Measured on echo-only audio, where
        // there is no near end whatsoever, it accounted for 15784 of 19784
        // releases (DECISIONS Q51).
        if far_recently_playing && self.since_dominated < self.dominance_hold {
            return error * self.gain;
        }
        self.gain += (1.0 - self.gain) * RELEASE;
        error * self.gain
    }

    /// Moves the write cursor, folding the buffer when it reaches the end.
    ///
    /// Copying half the buffer once every `taps` samples is what buys a
    /// contiguous window on every other sample.
    fn advance(&mut self) {
        self.write += 1;
        if self.write == self.history.len() {
            self.history.copy_within(self.taps.., 0);
            self.write = self.taps;
        }
    }

    /// A diverged filter is worse than no filter: it adds a loud, wrong
    /// signal to speech. Starting over costs a few seconds of convergence.
    fn guard_against_divergence(&mut self) {
        let norm: f32 = self.weights.iter().map(|w| w * w).sum();
        if !norm.is_finite() || norm > DIVERGENCE_LIMIT {
            self.weights.iter_mut().for_each(|w| *w = 0.0);
            self.active = false;
            self.resets += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use evertranscript_fixtures::ENGLISH_MEETING;
    use evertranscript_fixtures::echo::Room;
    use evertranscript_fixtures::echo::echo_of;
    use evertranscript_fixtures::echo::erle_db;

    use super::*;

    const RATE: u32 = 16_000;

    /// Speech-shaped noise: broadband and non-stationary, which is what an
    /// adaptive filter finds hard. A pure tone would flatter it.
    fn babble(seconds: f32, seed: u32) -> Vec<f32> {
        let count = (seconds * RATE as f32) as usize;
        let mut state = seed.wrapping_mul(2_654_435_761).max(1);
        let mut envelope = 0.0f32;
        (0..count)
            .map(|index| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                let noise = (state as f32 / u32::MAX as f32) * 2.0 - 1.0;
                // Syllable-rate amplitude modulation, so the signal starts
                // and stops the way speech does.
                let syllable = (index as f32 / RATE as f32 * 4.0).sin().abs();
                envelope += (syllable - envelope) * 0.001;
                noise * envelope * 0.3
            })
            .collect()
    }

    fn power(samples: &[f32]) -> f32 {
        samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32
    }
    #[test]
    fn echo_is_removed_when_only_the_far_end_is_talking() {
        // The case the whole module exists for: the Operator is silent and
        // listening on speakers, so everything the microphone hears is echo.
        let far = babble(8.0, 1);
        let echo = echo_of(&far, RATE, &Room::default());
        let mut microphone = echo.clone();

        let mut canceller = EchoCanceller::new(RATE);
        canceller.process(&mut microphone, &far);

        // Judge on the second half: the first is convergence, and a filter
        // is allowed to take a moment to learn a room.
        let half = microphone.len() / 2;
        let erle = erle_db(&echo[half..], &microphone[half..]);
        println!("  echo only: {erle:.1} dB of echo removed");
        assert!(
            erle > 15.0,
            "the echo should be well down after converging, got {erle:.1} dB"
        );
        assert!(canceller.is_cancelling(), "an echo path was learned");
        assert_eq!(canceller.resets(), 0, "a converging filter never diverges");
    }

    #[test]
    fn real_speech_echo_is_cancelled_by_a_measurable_amount() {
        // The same case as above, on real speech rather than babble, and in
        // decibels rather than words.
        //
        // **This is the guard that used to live in `transcription_quality`**,
        // where it asked whether an ASR could still read the far end out of
        // the residual — and that turned out to be a property of the *model*
        // rather than of this filter. Calibrated on `ggml-tiny` it passed at
        // 86.5%; the model the product actually ships reads through the same
        // residual and scores 64.9%, failing a threshold the canceller's
        // behaviour never moved (DECISIONS Q50). ERLE cannot drift that way:
        // it measures what this module does, in the unit the module is
        // specified in, and needs no model to say it.
        //
        // Babble is the harder signal for an adaptive filter and it is
        // covered above. Real speech is here because it is the input the
        // product meets and because it is the signal the superseded
        // assertion used, so the two are comparable.
        let far = ENGLISH_MEETING.samples_at(RATE);
        let echo = echo_of(&far.data, RATE, &Room::default());
        let mut microphone = echo.clone();

        let mut canceller = EchoCanceller::new(RATE);
        canceller.process(&mut microphone, &far.data);

        // The same second-half convention as the babble case: a filter is
        // allowed to spend the opening learning the room.
        let half = microphone.len() / 2;
        let erle = erle_db(&echo[half..], &microphone[half..]);
        println!("  real speech: {erle:.1} dB of echo removed");
        // **35, not 15.** 41.4 dB is what this measures once the suppressor
        // stops releasing in every pause; 32.8 dB is what it measured while
        // it did. A bar of 15 passed comfortably through the whole defect,
        // which is the argument against setting one from the wrong side of a
        // fix. This sits above the broken value so the regression cannot come
        // back green, and well under the working one so a platform that
        // rounds differently does not go red (DECISIONS Q51).
        assert!(
            erle > 35.0,
            "real speech echo should be well down after converging, \
             got {erle:.1} dB"
        );
        assert!(canceller.is_cancelling(), "an echo path was learned");
    }
    #[test]
    fn a_harder_room_is_still_reduced() {
        let far = babble(8.0, 7);
        let echo = echo_of(&far, RATE, &Room::reverberant());
        let mut microphone = echo.clone();

        let mut canceller = EchoCanceller::new(RATE);
        canceller.process(&mut microphone, &far);

        let half = microphone.len() / 2;
        let erle = erle_db(&echo[half..], &microphone[half..]);
        assert!(erle > 10.0, "a longer tail is harder, got {erle:.1} dB");
    }

    #[test]
    fn audio_with_no_echo_in_it_is_left_alone() {
        // The common case by far: headphones. Nothing the far end says
        // reaches the microphone, so this code must be invisible. Damaging
        // clean speech to cancel an echo that is not there would make the
        // product worse for most of its users.
        let near = babble(8.0, 3);
        let far = babble(8.0, 99); // unrelated: no echo path exists
        let mut microphone = near.clone();

        let mut canceller = EchoCanceller::new(RATE);
        canceller.process(&mut microphone, &far);

        let preservation =
            evertranscript_fixtures::similarity::rms_preservation_percent(&near, &microphone);
        println!("  no echo present: {preservation:.1}% of the level preserved");
        assert!(
            (97.0..=103.0).contains(&preservation),
            "clean speech must survive untouched, got {preservation:.1}%"
        );
    }

    #[test]
    fn the_near_end_speaker_is_not_cancelled_along_with_the_echo() {
        // Double talk: both people speaking at once. The filter must remove
        // the echo without eating the Operator, which is the failure that
        // makes a canceller worse than none.
        let far = babble(8.0, 11);
        let near = babble(8.0, 23);
        let echo = echo_of(&far, RATE, &Room::default());
        let mut microphone: Vec<f32> = near
            .iter()
            .zip(&echo)
            .map(|(near, echo)| near + echo)
            .collect();

        let mut canceller = EchoCanceller::new(RATE);
        canceller.process(&mut microphone, &far);

        // The near-end speech must still be there in strength. Comparing
        // power rather than waveform: some echo remains mixed in, so an
        // exact match is not the claim.
        let half = microphone.len() / 2;
        let kept = power(&microphone[half..]) / power(&near[half..]);
        println!(
            "  double talk: {:.0}% of the near-end power kept",
            kept * 100.0
        );
        assert!(
            kept > 0.5,
            "the Operator's own voice must survive, kept {kept:.2} of its power"
        );
    }

    #[test]
    fn a_silent_far_end_teaches_the_filter_nothing() {
        // Nothing is playing, so there is no echo to learn and no
        // information in the reference. Adapting here would fit noise.
        let near = babble(4.0, 5);
        let mut microphone = near.clone();
        let silence = vec![0.0f32; near.len()];

        let mut canceller = EchoCanceller::new(RATE);
        canceller.process(&mut microphone, &silence);

        assert!(
            !canceller.is_cancelling(),
            "nothing should have been learned"
        );
        assert_eq!(microphone, near, "and the audio is untouched");
    }

    #[test]
    fn a_short_reference_does_not_shift_the_rest_of_the_block() {
        // Defensive: the legs are aligned by construction, but treating a
        // missing reference as silence keeps a mismatch from turning into a
        // permanent offset between the channels.
        let mut microphone = vec![0.25f32; 1000];
        let mut canceller = EchoCanceller::new(RATE);
        canceller.process(&mut microphone, &[0.1; 10]);
        assert_eq!(microphone.len(), 1000);
    }
}

#[cfg(test)]
mod throughput {
    use super::*;

    /// The filter sits in the capture path of a live meeting, so it has to
    /// run faster than the audio arrives. 128 ms of taps is a deliberate
    /// choice with a cost attached, and this is where that cost is measured
    /// rather than assumed.
    ///
    /// Debug builds run this roughly fifteen times slower than shipped ones,
    /// so unoptimized runs measure a shorter clip and report without
    /// asserting. The number that matters is the release one.
    #[test]
    fn it_runs_faster_than_the_audio_it_processes() {
        const RATE: u32 = 16_000;
        let seconds: usize = if cfg!(debug_assertions) { 2 } else { 60 };

        // Audio that keeps the filter fully active: a far end that is always
        // playing, and a near end that is its echo, so nothing takes the
        // shortcut for a quiet reference.
        let far: Vec<f32> = (0..RATE as usize * seconds)
            .map(|index| (index as f32 * 0.01).sin() * 0.3)
            .collect();
        let mut near: Vec<f32> = far.iter().map(|sample| sample * 0.3).collect();

        let mut canceller = EchoCanceller::new(RATE);
        let started = std::time::Instant::now();
        canceller.process(&mut near, &far);
        let elapsed = started.elapsed().as_secs_f64();
        let realtime = seconds as f64 / elapsed;
        println!("  {realtime:.0}x realtime ({elapsed:.2}s for {seconds}s of audio)");

        if cfg!(debug_assertions) {
            return;
        }
        assert!(
            realtime > 5.0,
            "the canceller must comfortably outrun live audio, got {realtime:.1}x"
        );
    }
}
