//! Judging an enrolment recording, and turning it into evidence.
//!
//! Three questions, and the product's whole trust in rule 0 rests on all
//! three being asked before anything is written. An enrolment outranks every
//! other rule in `super::operator`, so a bad one is worse than none: it does
//! not degrade the identification, it replaces it with a confident wrong
//! answer that no later Meeting can argue with.
//!
//! 1. **Did anything arrive?** On macOS a refused microphone reports the
//!    grant as given and delivers zeros forever, so silence is the shape a
//!    permission failure takes rather than an unusual recording.
//! 2. **Is there enough of it?** [`super::operator::MIN_OPERATOR_MS`]
//!    already means exactly "enough of one voice to be worth enrolling",
//!    and it is reused rather than restated: two floors for one idea drift.
//! 3. **Is it one voice?** The question this whole feature exists to
//!    settle. An enrolment recorded with somebody else talking would put
//!    that person inside the Operator's identity permanently and by the
//!    Operator's own act, which is the failure of `Identified::IsolatedMic`
//!    (Q248, Q281) with the one safeguard — a threshold — removed.
//!
//! The spans that come out are the diarizer's own turns, so the enrolment is
//! cut the way every other exemplar in the record is cut, by the same model.

use evertranscript_protocol::AudioChannel;

use super::Cancel;
use super::Diarization;
use super::DiarizeError;
use super::Diarizer;
use super::MeetingAudio;
use super::Turn;
use super::fbank::SAMPLE_RATE;
use super::live::MAX_SPAN_MS;
use super::live::MIN_SPAN_MS;
use super::operator::MIN_OPERATOR_MS;
use super::runner::Embed;
use crate::store::speakers::EnrolmentSpan;

/// Why a recording was not accepted as an enrolment.
///
/// Each variant is something the Operator can do differently, which is the
/// point of keeping them apart: "that did not work" sends a person to
/// Settings, and "there was somebody else talking" sends them somewhere
/// quieter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// Every sample was zero.
    Silent,
    /// Some speech, but not enough of it to be an identity.
    TooLittleSpeech { voiced_ms: u64, needed_ms: u64 },
    /// More than one person is in the recording.
    MoreThanOneVoice { voices: usize },
    /// One voice, long enough, and the model could not embed any span of it.
    NothingEmbeddable,
}

impl Refused {
    /// A stable tag for the protocol, so the Client chooses its own words.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Silent => "silent",
            Self::TooLittleSpeech { .. } => "too-little-speech",
            Self::MoreThanOneVoice { .. } => "more-than-one-voice",
            Self::NothingEmbeddable => "nothing-embeddable",
        }
    }
}

/// An accepted enrolment, ready to be written.
#[derive(Debug, Clone, PartialEq)]
pub struct Accepted {
    pub spans: Vec<EnrolmentSpan>,
    pub voiced_ms: u64,
}

/// What [`analyse`] made of a recording.
///
/// A refusal is an outcome and not an error. The models ran, they answered,
/// and the answer is that this recording should not become an identity —
/// which the Operator can act on. [`DiarizeError`] stays what it is: the
/// model could not be loaded or could not run, and nobody can do anything
/// about it by speaking differently.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Accepted(Accepted),
    Refused(Refused),
}

/// Runs the segmenter over an enrolment recording.
///
/// Split from [`analyse`] because the two models are reached through one
/// `&mut`: `LiveDiarizer` owns its embedder, so a caller cannot hold the
/// diarizer and hand out an embed closure at the same time. Sequencing them
/// is what the Meeting path already does (`runner::rebuild` then
/// `Diarizer::diarize`, server.rs), and it is also what lets a test drive
/// this half from `super::fixture::FixtureDiarizer`.
///
/// The far end is passed as silence rather than as nothing. There is no far
/// end in an enrolment, and silence is what that looks like to every model
/// downstream; an empty slice would be a second, untested shape of input for
/// no gain.
pub fn listen(mic: &[f32], diarizer: &mut dyn Diarizer) -> Result<Diarization, DiarizeError> {
    let silence = vec![0.0_f32; mic.len()];
    diarizer.diarize(
        MeetingAudio {
            mic,
            system: &silence,
            sample_rate: SAMPLE_RATE,
        },
        &mut |_| {},
        &Cancel::new(),
    )
}

/// Judges what [`listen`] heard, and either accepts it or says why not.
///
/// `mic` is the clip at [`SAMPLE_RATE`], as `super::runner::decode` returns
/// it — the same decode a Meeting goes through, so an enrolment re-read from
/// disk months later is judged by the same arithmetic as one just recorded.
///
/// `embed` is a closure rather than the diarizer's own embedder because the
/// embedder that matters is the one the *caller* is about to write vectors
/// with, which after a model change is not the one that produced the last
/// ones. `runner::rebuild` and `super::reseed::embed_ranges` take the same
/// shape for the same reason.
pub fn analyse(
    mic: &[f32],
    diarization: &Diarization,
    embed: &mut Embed<'_>,
) -> Result<Outcome, DiarizeError> {
    let voiced_ms: u64 = diarization.turns.iter().map(Turn::duration_ms).sum();
    let voices = diarization.clusters().len();
    if let Some(refused) = refuse(mic, voiced_ms, voices) {
        return Ok(Outcome::Refused(refused));
    }

    let mut spans = Vec::new();
    for turn in &diarization.turns {
        let Some((start_ms, end_ms)) = bounded(turn) else {
            continue;
        };
        let (from, to) = (
            samples_at(start_ms, mic.len()),
            samples_at(end_ms, mic.len()),
        );
        if to <= from {
            continue;
        }
        if let Some(vector) = embed(&mic[from..to])? {
            spans.push(EnrolmentSpan {
                vector,
                voiced_ms: (end_ms - start_ms) as i64,
                start_ms: start_ms as i64,
                end_ms: end_ms as i64,
            });
        }
    }
    if spans.is_empty() {
        return Ok(Outcome::Refused(Refused::NothingEmbeddable));
    }
    Ok(Outcome::Accepted(Accepted { spans, voiced_ms }))
}

/// The three gates, apart from the models so they can be checked without
/// one. Order matters only for which reason a bad recording reports, and
/// silence comes first because it is the one that is usually a permission.
pub fn refuse(mic: &[f32], voiced_ms: u64, voices: usize) -> Option<Refused> {
    if mic.iter().all(|sample| *sample == 0.0) {
        return Some(Refused::Silent);
    }
    if voices > 1 {
        return Some(Refused::MoreThanOneVoice { voices });
    }
    if voiced_ms < MIN_OPERATOR_MS {
        return Some(Refused::TooLittleSpeech {
            voiced_ms,
            needed_ms: MIN_OPERATOR_MS,
        });
    }
    None
}

/// One turn, clipped to a span the embedder will take.
///
/// The same bounds the live pass uses, so an enrolment's exemplars are the
/// same size as everyone else's and the centroid weighting compares like
/// with like.
fn bounded(turn: &Turn) -> Option<(u64, u64)> {
    if turn.channel != AudioChannel::Mic {
        return None;
    }
    let start_ms = turn.start.millis();
    let end_ms = turn.end.millis().min(start_ms + MAX_SPAN_MS);
    (end_ms.saturating_sub(start_ms) >= MIN_SPAN_MS).then_some((start_ms, end_ms))
}

fn samples_at(millis: u64, len: usize) -> usize {
    ((millis * SAMPLE_RATE as u64 / 1000) as usize).min(len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diarize::fixture::FixtureDiarizer;

    /// Ten seconds of something, at the rate the models run at.
    ///
    /// Non-zero because [`refuse`] reads all-zero as a refused microphone,
    /// and the fixture diarizer answers from its script rather than from
    /// what is in the buffer — so the content only has to be *not silence*.
    fn audible(seconds: usize) -> Vec<f32> {
        vec![0.5_f32; seconds * SAMPLE_RATE as usize]
    }

    /// An embedder that returns a fixed vector and records what it was
    /// shown, so a test can assert on the spans the model was actually
    /// handed. The pattern `super::reseed`'s tests already use.
    fn embedder(
        seen: &mut Vec<usize>,
    ) -> impl FnMut(&[f32]) -> Result<Option<Vec<f32>>, DiarizeError> + '_ {
        move |samples| {
            seen.push(samples.len());
            Ok(Some(vec![1.0, samples.len() as f32]))
        }
    }

    #[test]
    fn one_voice_is_accepted_with_the_spans_the_diarizer_cut() {
        // Ten seconds of buffer against a fifty-minute script: every turn
        // past the first falls off the end of the audio, which is the case
        // `samples_at`'s clamp exists for. One span comes back, and it is
        // the first turn clipped to what the embedder takes.
        let mic = audible(10);
        let mut diarizer = FixtureDiarizer::solo();
        let heard = listen(&mic, &mut diarizer).expect("listen");

        let mut seen = Vec::new();
        let outcome = analyse(&mic, &heard, &mut embedder(&mut seen)).expect("analyse");

        let Outcome::Accepted(accepted) = outcome else {
            panic!("expected an acceptance, got {outcome:?}");
        };
        assert_eq!(accepted.spans.len(), 1);
        assert_eq!(accepted.spans[0].start_ms, 0);
        assert_eq!(accepted.spans[0].end_ms, MAX_SPAN_MS as i64);
        assert_eq!(accepted.spans[0].voiced_ms, MAX_SPAN_MS as i64);
        assert_eq!(seen, vec![mic.len()], "the model saw the span, whole");

        // Voiced time, not wall clock: the number that meets
        // `MIN_OPERATOR_MS` is the sum of the diarizer's turns.
        assert_eq!(
            accepted.voiced_ms,
            heard.turns.iter().map(Turn::duration_ms).sum::<u64>()
        );
        assert!(accepted.voiced_ms >= MIN_OPERATOR_MS);
    }

    #[test]
    fn a_colleague_in_the_room_is_refused_out_of_the_diarization_itself() {
        // The case this whole feature exists for, reached the way the
        // product reaches it — a real run over a shared-room timeline,
        // counting the clusters that came back, rather than a count handed
        // to `refuse` by the test.
        let mic = audible(20);
        let mut diarizer = FixtureDiarizer::shared_room();
        let heard = listen(&mic, &mut diarizer).expect("listen");

        let mut seen = Vec::new();
        assert_eq!(
            analyse(&mic, &heard, &mut embedder(&mut seen)).expect("analyse"),
            Outcome::Refused(Refused::MoreThanOneVoice { voices: 3 })
        );
        assert!(
            seen.is_empty(),
            "a refused recording is never embedded: the refusal comes first"
        );
    }

    #[test]
    fn one_voice_the_model_cannot_embed_is_refused_rather_than_enrolled_empty() {
        // `Ok(None)` is what the real embedder returns for a span it has too
        // little of. Every span answering that way leaves nothing to be an
        // identity, and writing that would enrol the Operator as a Speaker
        // with no vectors — recognized by rule 0 against nothing at all.
        let mic = audible(10);
        let mut diarizer = FixtureDiarizer::solo();
        let heard = listen(&mic, &mut diarizer).expect("listen");

        assert_eq!(
            analyse(&mic, &heard, &mut |_| Ok(None)).expect("analyse"),
            Outcome::Refused(Refused::NothingEmbeddable)
        );
    }

    #[test]
    fn a_refused_microphone_reads_as_silence_rather_than_as_a_short_recording() {
        // The macOS case: the grant is reported as given, and every sample
        // is zero. Reported as silence even though it is also, trivially,
        // too little speech — the Operator's next move is System Settings,
        // not talking for longer.
        assert_eq!(refuse(&[0.0; 48_000], 0, 0), Some(Refused::Silent));
    }

    #[test]
    fn a_second_voice_refuses_the_whole_recording() {
        // Not "use the dominant one". A dominant share is what
        // `operator::identify` rule 2 already does from a Meeting, and
        // taking it here would put the guess back inside the act that
        // exists to replace it — with no threshold, and permanently.
        assert_eq!(
            refuse(&[0.5; 48_000], 60_000, 2),
            Some(Refused::MoreThanOneVoice { voices: 2 })
        );
    }

    #[test]
    fn the_floor_is_the_one_the_rules_already_use() {
        assert_eq!(
            refuse(&[0.5; 48_000], MIN_OPERATOR_MS - 1, 1),
            Some(Refused::TooLittleSpeech {
                voiced_ms: MIN_OPERATOR_MS - 1,
                needed_ms: MIN_OPERATOR_MS,
            })
        );
        assert_eq!(refuse(&[0.5; 48_000], MIN_OPERATOR_MS, 1), None);
    }

    #[test]
    fn a_turn_longer_than_the_model_takes_is_clipped_rather_than_dropped() {
        let long = Turn::new(AudioChannel::Mic, 1_000, 1_000 + MAX_SPAN_MS * 3, 0);
        assert_eq!(bounded(&long), Some((1_000, 1_000 + MAX_SPAN_MS)));

        let brief = Turn::new(AudioChannel::Mic, 0, MIN_SPAN_MS - 1, 0);
        assert_eq!(bounded(&brief), None, "too short to embed");

        // There is no far end in an enrolment, but the far end is passed as
        // silence and the diarizer is free to find a turn in it.
        let elsewhere = Turn::new(AudioChannel::System, 0, MAX_SPAN_MS, 0);
        assert_eq!(bounded(&elsewhere), None);
    }
}
