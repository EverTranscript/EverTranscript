//! Shared test fixtures: real speech, known ground truth, and the assertions
//! that make audio testable.
//!
//! This crate is the harness every later milestone rides. Three things it
//! deliberately provides, because each one is a trap otherwise:
//!
//! - **Real audio, not synthetic tones.** A sine wave proves a pipeline moves
//!   bytes; it proves nothing about VAD, chunking, or transcription. These
//!   clips are speech.
//! - **Similarity by features, not bit-exactness.** Audio that has been
//!   resampled, encoded, and decoded is never bit-identical to its input, so
//!   an equality assertion is either always false or so loose it tests
//!   nothing. Compare RMS, peak, zero-crossing rate, and spectral shape with
//!   explicit tolerances instead.
//! - **Ground truth beside the audio.** Transcription quality is a number
//!   (WER/CER) that has to be tracked over time, not a vibe.
//!
//! **About the speech.** These clips are synthesized with macOS `say`, which
//! buys reproducibility, exact ground truth, and no licensing question. It
//! also makes them *easier* than real meetings: no room reverb, no crosstalk,
//! no accents, clean turn boundaries. They are the right fixtures for "does
//! the pipeline work"; they are the wrong fixtures for "is transcription good
//! enough to ship". Real recorded meetings are separate homework, and the
//! PRD's ASR-quality risk is not retired by a good number here.

pub mod echo;
pub mod similarity;
pub mod wer;

/// A fixture clip with everything a test needs to judge the result.
#[derive(Debug, Clone, Copy)]
pub struct Fixture {
    pub name: &'static str,
    /// 16-bit mono PCM WAV.
    pub wav: &'static [u8],
    /// What is actually said, for WER/CER. Empty when the clip has no speech.
    pub transcript: &'static str,
    /// What this clip is for.
    pub purpose: &'static str,
}

/// Two English speakers, distinct voices — the everyday case, and the
/// material M3's diarization has to separate.
pub const ENGLISH_MEETING: Fixture = Fixture {
    name: "english_meeting",
    wav: include_bytes!("../assets/english_meeting.wav"),
    transcript: "So where are we on the budget review? I think we agreed to defer the hiring \
                 plan until October. That's right. I'll send the revised numbers before Friday \
                 so the team can look them over.",
    purpose: "two-speaker English meeting",
};

/// Mandarin and English in one meeting. The PRD's story 7 (code-switching)
/// is not an edge case for this product's Operator — it is the common case,
/// and it is what drove the large-v3-turbo model choice.
pub const BILINGUAL_MEETING: Fixture = Fixture {
    name: "bilingual_meeting",
    wav: include_bytes!("../assets/bilingual_meeting.wav"),
    transcript: "我们下周的预算评审会议改到周三，麻烦大家把材料提前发给我。 Got it. I'll prepare \
                 the slides and share them in the group chat before Wednesday.",
    purpose: "Mandarin/English code-switching",
};

/// One voice, alone, for half a minute — an enrolment sample.
///
/// Everything else here is a *meeting*: two voices, short turns, the material
/// diarization has to pull apart. This is the opposite shape and exists for
/// the opposite reason. `diarize::enrol` refuses a recording that holds more
/// than one voice or less than `operator::MIN_OPERATOR_MS` of speech, so the
/// only clip that can prove an enrolment is *accepted* is one that is neither,
/// and no such clip existed: `english_meeting` is two people and too short per
/// person, and the AMI corpus ships a mixed headset.
///
/// Continuous on purpose. The floor is twenty seconds of *voiced* audio as
/// the diarizer counts turns, not twenty seconds of wall clock, and a clip
/// with a reader's pauses in it would sit near that line rather than over it.
pub const OPERATOR_ENROLMENT: Fixture = Fixture {
    name: "operator_enrolment",
    wav: include_bytes!("../assets/operator_enrolment.wav"),
    transcript: "This is a sample of my speaking voice, recorded so that the application can \
                 tell which voice in a meeting belongs to me. I am reading a short passage \
                 aloud, at a normal pace, in the room where I usually work, so that the \
                 recording sounds like the meetings it will be compared against. There is \
                 nobody else here and nothing else playing. When this finishes, the application \
                 will listen to what I said, check that it heard exactly one person, and keep a \
                 short recording along with the measurements it takes from it. If the voice \
                 model is ever replaced, that kept recording is what lets it recognise me again \
                 without going back through every meeting I have ever had.",
    purpose: "the Operator's enrolment sample: one voice, no far end, over the MIN_OPERATOR_MS floor",
};

/// Digital silence. The classic Whisper hallucination trigger: given nothing,
/// it confidently produces "Thank you for watching." Our record is immutable,
/// so a hallucination here persists in History forever (ticket 07).
pub const SILENCE: Fixture = Fixture {
    name: "silence",
    wav: include_bytes!("../assets/silence.wav"),
    transcript: "",
    purpose: "hallucination canary: silence must transcribe to nothing",
};

/// Low-level pink noise — an empty room with the mic open. The other shape
/// of the same failure.
pub const ROOM_NOISE: Fixture = Fixture {
    name: "room_noise",
    wav: include_bytes!("../assets/room_noise.wav"),
    transcript: "",
    purpose: "hallucination canary: room tone must transcribe to nothing",
};

pub const ALL: &[Fixture] = &[
    ENGLISH_MEETING,
    BILINGUAL_MEETING,
    OPERATOR_ENROLMENT,
    SILENCE,
    ROOM_NOISE,
];

/// Clips that must produce no transcript at all.
pub const CANARIES: &[Fixture] = &[SILENCE, ROOM_NOISE];

/// Decoded audio: mono `f32` in `[-1, 1]`, with its sample rate.
#[derive(Debug, Clone)]
pub struct Samples {
    pub rate: u32,
    pub data: Vec<f32>,
}

impl Samples {
    pub fn duration_seconds(&self) -> f64 {
        self.data.len() as f64 / self.rate as f64
    }

    /// Nearest-sample resampling, for producing a fixture at another rate.
    ///
    /// Deliberately crude: this exists so tests can feed 48 kHz capture from
    /// 16 kHz assets, not to be the product's resampler (that is ticket 08's
    /// persistent sinc implementation).
    pub fn resampled(&self, rate: u32) -> Samples {
        if rate == self.rate || self.data.is_empty() {
            return self.clone();
        }
        let out_len = (self.data.len() as u64 * rate as u64 / self.rate as u64) as usize;
        let data = (0..out_len)
            .map(|index| {
                let source = (index as u64 * self.rate as u64 / rate as u64) as usize;
                self.data[source.min(self.data.len() - 1)]
            })
            .collect();
        Samples { rate, data }
    }
}

impl Fixture {
    /// Decodes the clip at its stored rate (16 kHz).
    pub fn samples(&self) -> Samples {
        let reader = hound::WavReader::new(std::io::Cursor::new(self.wav))
            .unwrap_or_else(|error| panic!("fixture {} is not readable: {error}", self.name));
        let spec = reader.spec();
        assert_eq!(spec.channels, 1, "fixtures are mono");

        let data: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Int => {
                let scale = 1.0 / (1i64 << (spec.bits_per_sample - 1)) as f32;
                reader
                    .into_samples::<i32>()
                    .map(|sample| sample.expect("sample") as f32 * scale)
                    .collect()
            }
            hound::SampleFormat::Float => reader
                .into_samples::<f32>()
                .map(|sample| sample.expect("sample"))
                .collect(),
        };
        Samples {
            rate: spec.sample_rate,
            data,
        }
    }

    /// Decodes at an arbitrary rate — capture runs at 48 kHz, ASR at 16 kHz,
    /// and tests need both from the same asset.
    pub fn samples_at(&self, rate: u32) -> Samples {
        self.samples().resampled(rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fixture_decodes_to_real_audio() {
        for fixture in ALL {
            let samples = fixture.samples();
            assert_eq!(samples.rate, 16_000, "{} should be 16 kHz", fixture.name);
            assert!(
                samples.duration_seconds() > 1.0,
                "{} is suspiciously short",
                fixture.name
            );
            assert!(
                samples.data.iter().all(|sample| sample.abs() <= 1.0),
                "{} has samples outside [-1, 1]",
                fixture.name
            );
        }
    }

    #[test]
    fn speech_fixtures_actually_contain_speech() {
        for fixture in [ENGLISH_MEETING, BILINGUAL_MEETING] {
            let samples = fixture.samples();
            let features = similarity::Features::of(&samples.data, samples.rate);
            assert!(
                features.rms > 0.01,
                "{} should have real signal, got rms {}",
                fixture.name,
                features.rms
            );
            assert!(
                !fixture.transcript.is_empty(),
                "{} needs ground truth to be useful",
                fixture.name
            );
        }
    }

    #[test]
    fn canaries_are_quiet_and_have_no_ground_truth() {
        for fixture in CANARIES {
            let samples = fixture.samples();
            let features = similarity::Features::of(&samples.data, samples.rate);
            assert!(
                features.rms < 0.05,
                "{} should be near-silent, got rms {}",
                fixture.name,
                features.rms
            );
            assert!(
                fixture.transcript.is_empty(),
                "a canary must expect no transcript"
            );
        }
    }

    #[test]
    fn resampling_to_the_capture_rate_preserves_duration() {
        let at_16k = ENGLISH_MEETING.samples();
        let at_48k = ENGLISH_MEETING.samples_at(48_000);
        assert_eq!(at_48k.rate, 48_000);
        assert!(
            (at_48k.duration_seconds() - at_16k.duration_seconds()).abs() < 0.01,
            "resampling must not change how long the clip is"
        );
    }
}
