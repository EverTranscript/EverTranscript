//! "You" — the Operator's own Speaker, and the channel prior that is only a
//! prior.
//!
//! ADR-0029 originally said the mic channel **is** the Operator, and was
//! amended to say it is *where the Operator is*. The difference is a whole
//! class of recordings: put two people in a conference room with one laptop
//! and the second voice is on the mic channel, real, and not the Operator.
//! A design that cannot represent that mis-attributes every word the person
//! across the table says, and does it silently — the transcript looks
//! perfectly plausible, it is just wrong about who was talking.
//!
//! So the channel narrows the field and never decides it. Four rules do,
//! in this order (ADR-0029 as amended):
//!
//! 0. **Enrolment.** The Operator recorded themselves and said so, and that
//!    recording matched a voice here. Every rule below it is a guess about
//!    who owns the laptop, made from a recording nobody confirmed; this one
//!    is a person's own account of their own voice, and it is the only
//!    evidence in this file that did not have to be inferred. **When an
//!    enrolment exists, it is the whole of the answer** — a miss means the
//!    Operator did not speak here, and the rules below do not get a turn.
//!    Falling through to them would re-admit exactly the guess the
//!    enrolment was recorded to replace.
//! 1. **Isolated mic.** The far end could not have reached the microphone —
//!    headphones were the only playing output and the microphone was never
//!    swapped — so every mic-channel voice is the Operator, confirmed
//!    without any act. A fact recorded at capture time, because the audio
//!    stops being able to say afterwards.
//! 2. **Dominance.** One voice held [`DOMINANCE`] of the mic channel, beat
//!    the runner-up by [`DOMINANCE_MARGIN`], and did it for at least
//!    [`MIN_OPERATOR_MS`]. Two people sharing a microphone evenly produce
//!    nothing at all, which is the honest outcome: the machine cannot know
//!    which of them owns the laptop, and guessing gives someone else's words
//!    the Operator's name.
//! 3. **Voiceprint match**, and only in a Meeting holding
//!    [`MIN_DIARIZED_MS`] of speech and [`MIN_SPEAKERS_FOR_MATCH`] voices.
//!    Below that gate the Voiceprint is withheld from the *whole* resolve
//!    rather than only from the flag — the general resolve carries it among
//!    every other seed and would match on it anyway, which would leave the
//!    gate looking like a rule and behaving like a comment.
//!
//! Dominance comes before the match because it needs no Voiceprint and
//! cannot be misled by one: a Meeting the Operator plainly dominates is the
//! Operator's, whatever History believes about a voice that sounds similar.
//!
//! **Rule 0 is not gated, and rule 3 is.** The gate exists because rule 3's
//! evidence is inferred: thirty seconds and one voice is a monologue, where
//! a match says nothing dominance does not already say better. Under rule 0
//! dominance never runs, so the gate would not be withholding a weaker
//! answer in favour of a stronger one — it would be refusing the only
//! answer there is, and a short solo Meeting would come back with no
//! Operator in it. The gate is about how far an inference may be trusted,
//! and an enrolment is not an inference.
//!
//! **There is only ever one.** Migration 14 makes a second flagged row a
//! constraint violation, and [`crate::store::speakers::set_operator`] moves
//! the flag rather than adding it.

use std::collections::BTreeMap;

use evertranscript_protocol::AudioChannel;

use super::Cluster;
use super::Diarization;
use super::Embedding;
use super::cluster::Resolved;
use super::cluster::SeedVoice;
use super::cluster::resolve;

/// How much of the mic channel one voice must hold to be assumed the
/// Operator.
///
/// Chosen for the shape of the failure rather than tuned: a solo recording
/// is ~100%, and a genuinely shared room is near half each. Anything in
/// between is ambiguous, and this product would rather leave the Operator
/// unidentified for one Meeting than name the wrong person — an unnamed
/// Speaker is a visible gap, a wrongly-named one is invisible.
pub const DOMINANCE: f32 = 0.80;

/// How far the leading mic voice must beat the runner-up, as a share of mic
/// time. Guards the case where one voice clears [`DOMINANCE`] only because
/// the others are numerous rather than quiet.
pub const DOMINANCE_MARGIN: f32 = 0.5;

/// How much of that one voice there must actually be.
///
/// A share is a ratio and says nothing about size: ten seconds of one person
/// talking to themselves is 100% of the mic channel, and enrolling a
/// permanent "You" from it is how a microphone test becomes a Speaker. The
/// share says the voice is alone; this says there was enough of it to be
/// worth keeping.
pub const MIN_OPERATOR_MS: u64 = 20_000;

/// How much diarized speech a Meeting must hold before a Voiceprint match
/// is allowed to name the Operator.
pub const MIN_DIARIZED_MS: u64 = 30_000;

/// And how many voices. One voice and thirty seconds is a monologue, where
/// a match says nothing the dominance rule does not already say better.
pub const MIN_SPEAKERS_FOR_MATCH: usize = 2;

/// What the recording itself says, beyond the audio.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MeetingFacts {
    /// The far end could not have reached the microphone: headphones were
    /// the only playing output and the microphone was never swapped. `None`
    /// where the capture layer never said.
    pub mic_isolated: Option<bool>,
}

/// Which clusters are the Operator, and which rule said so.
///
/// The rule is part of the answer rather than a log line because the three
/// differ in how much they are trusted downstream, and a caller that only
/// got `Option<Cluster>` back could not tell a fact about the recording —
/// rule 1, which needs no threshold — from a thresholded judgement about
/// who spoke most, or from a match made on evidence out of History.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identified {
    /// Rule 0. The voice the Operator enrolled is in this Meeting. The only
    /// outcome here resting on an act rather than on a reading of the
    /// recording, and so the only one that cannot be wrong about a
    /// colleague: somebody else's voice does not match the clip.
    Enrolled(Cluster),
    /// Rule 1. Nothing but the room reached the microphone and exactly one
    /// voice was on it, so that voice is the Operator, confirmed without any
    /// act. A second voice on an isolated microphone is a person in the
    /// room, not the Operator, and the rule refuses rather than enrolling
    /// them — see [`identify`]. The payload stays a `Vec` because
    /// `attach_operator` still has to survive being handed several.
    IsolatedMic(Vec<Cluster>),
    /// Rule 2. One voice held the mic channel, by share, by margin, and for
    /// long enough to be worth enrolling.
    Dominant(Cluster),
    /// Rule 3. The Operator's Voiceprint matched, in a Meeting big enough
    /// for that to mean something.
    Recognized(Cluster),
    /// No rule fired, which is an answer and not a failure.
    Nobody,
}

impl Identified {
    /// Every cluster this names, in order. Empty for [`Identified::Nobody`].
    pub fn clusters(&self) -> &[Cluster] {
        match self {
            Identified::IsolatedMic(clusters) => clusters,
            Identified::Enrolled(cluster)
            | Identified::Dominant(cluster)
            | Identified::Recognized(cluster) => std::slice::from_ref(cluster),
            Identified::Nobody => &[],
        }
    }
}

/// Whether a Voiceprint match is admissible evidence in this Meeting.
///
/// Below the gate the Operator's Voiceprint is withheld from the **whole**
/// resolve, not merely from the flag — see [`identify`]. Exposed because the
/// caller has to know before it assembles the seeds, which happens well
/// before anyone asks who the Operator is.
pub fn match_gate_met(diarization: &Diarization) -> bool {
    let speech: u64 = diarization
        .turns
        .iter()
        .map(|turn| turn.duration_ms())
        .sum();
    speech >= MIN_DIARIZED_MS && diarization.clusters().len() >= MIN_SPEAKERS_FOR_MATCH
}

/// How the Operator's Voiceprint came to exist.
///
/// Two vectors of the same shape that mean entirely different things, which
/// is why this is a type and not a `bool` beside the seed. A caller holding
/// `Option<&SeedVoice>` and a flag can pass the flag of one and the vector
/// of the other; this cannot be assembled wrongly.
#[derive(Debug, Clone, Copy)]
pub enum OperatorPrint<'a> {
    /// From an act: the Operator recorded themselves. Rule 0 — decides
    /// outright, ungated, and nothing below it runs.
    Enrolled(&'a SeedVoice),
    /// From the record, by inference over past Meetings. Rule 3's evidence,
    /// and only where [`match_gate_met`] holds — the caller withholds it
    /// otherwise, because the general resolve would match on it anyway and
    /// leave the gate decorative.
    Learned(Option<&'a SeedVoice>),
}

/// Which clusters are the Operator, by the four rules in order.
///
/// `print` decides which rules run at all: an [`OperatorPrint::Enrolled`]
/// answers on its own, and only [`OperatorPrint::Learned`] reaches the three
/// rules that read the recording. See the module documentation for why a
/// miss under rule 0 is [`Identified::Nobody`] rather than a fall-through.
pub fn identify(
    diarization: &Diarization,
    print: OperatorPrint<'_>,
    facts: &MeetingFacts,
) -> Identified {
    // 0. The Operator's own recording of their own voice. Not gated, not
    //    ordered behind the channel, and not followed by anything: the
    //    rules below exist to guess at what this one was told.
    let known = match print {
        OperatorPrint::Enrolled(enrolled) => {
            return match matching_mic_cluster(diarization, enrolled) {
                Some(cluster) => Identified::Enrolled(cluster),
                None => Identified::Nobody,
            };
        }
        OperatorPrint::Learned(known) => known,
    };

    // 1. The microphone heard the room and nothing else, and one voice was
    //    on it. There is no inference to make and nothing for a threshold to
    //    get wrong.
    //
    //    A second voice on that microphone is somebody in the room, and this
    //    is the only rule that would enrol them with no act at all. Naming
    //    them "You" is being confidently wrong about another person (Q248),
    //    in exactly the case no window filter can reach (Q266) — so more
    //    than one cluster falls through to rule 2, which is thresholded and
    //    already refuses a colleague. The isolated fact is still true; it
    //    just stops being sufficient once it names more than one voice.
    if facts.mic_isolated == Some(true) {
        let mut clusters: Vec<Cluster> = diarization
            .turns
            .iter()
            .filter(|turn| turn.channel == AudioChannel::Mic)
            .map(|turn| turn.cluster)
            .collect();
        clusters.sort_unstable();
        clusters.dedup();
        if clusters.len() == 1 {
            return Identified::IsolatedMic(clusters);
        }
    }

    // 2. One voice held the microphone. Before rule 3 because it needs no
    //    Voiceprint and cannot be fooled by one: a Meeting the Operator
    //    dominates is the Operator's, whoever else History thinks it knows.
    if let Some(leader) = dominant(diarization) {
        return Identified::Dominant(leader);
    }

    // 3. A Voiceprint matched. Last because it is the only rule whose
    //    evidence comes from outside this recording.
    if let Some(known) = known
        && let Some(cluster) = matching_mic_cluster(diarization, known)
    {
        return Identified::Recognized(cluster);
    }

    Identified::Nobody
}

/// The mic-channel cluster this seed matches, where one does.
///
/// Shared by rules 0 and 3, which differ in what the seed *means* and not at
/// all in how it is compared: the same `resolve`, at the same threshold,
/// against the same candidates. Two copies would be two places for the
/// threshold to drift, and a rule-0 match that scored differently from a
/// rule-3 match would be very hard to explain to anyone.
fn matching_mic_cluster(diarization: &Diarization, seed: &SeedVoice) -> Option<Cluster> {
    let mic_clusters: BTreeMap<Cluster, Embedding> = diarization
        .embeddings
        .iter()
        .filter(|(cluster, _)| speaks_on_mic(diarization, **cluster))
        .map(|(cluster, embedding)| (*cluster, embedding.clone()))
        .collect();
    resolve(&mic_clusters, std::slice::from_ref(seed))
        .into_iter()
        .find_map(|(cluster, outcome)| {
            matches!(outcome, Resolved::Existing(ref id) if *id == seed.speaker_id)
                .then_some(cluster)
        })
}

/// The voice that held the mic channel, where one did.
fn dominant(diarization: &Diarization) -> Option<Cluster> {
    let mut mic_time: BTreeMap<Cluster, u64> = BTreeMap::new();
    for turn in &diarization.turns {
        if turn.channel == AudioChannel::Mic {
            *mic_time.entry(turn.cluster).or_default() += turn.duration_ms();
        }
    }

    let total: u64 = mic_time.values().sum();
    if total == 0 {
        return None;
    }

    let mut ranked: Vec<(Cluster, u64)> = mic_time.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let (leader, leader_ms) = *ranked.first()?;
    if leader_ms < MIN_OPERATOR_MS {
        return None;
    }
    let share = leader_ms as f32 / total as f32;
    if share < DOMINANCE {
        return None;
    }

    // Clearing the share is not enough on its own: five quiet voices and one
    // moderate one can produce a leader with no real claim.
    let runner_up = ranked.get(1).map(|(_, ms)| *ms).unwrap_or(0);
    if (leader_ms as f32 - runner_up as f32) / total as f32 <= DOMINANCE_MARGIN && ranked.len() > 1
    {
        return None;
    }

    Some(leader)
}

fn speaks_on_mic(diarization: &Diarization, cluster: Cluster) -> bool {
    diarization
        .turns
        .iter()
        .any(|turn| turn.cluster == cluster && turn.channel == AudioChannel::Mic)
}

/// Ensures a Speaker marked as the Operator exists, returning its id.
///
/// "You" is a display name, not a magic record (ticket 05): this creates an
/// ordinary Speaker row with `is_operator` set, so it appears in the Voice
/// Registry, can be renamed, and can have its Voiceprint deleted like any
/// other. The flag decides what it is *called* by default, nothing else.
pub fn ensure_operator_speaker(connection: &rusqlite::Connection) -> anyhow::Result<String> {
    if let Some(existing) = crate::store::speakers::operator(connection)? {
        return Ok(existing.id);
    }
    Ok(crate::store::speakers::create(connection, true)?.id)
}

/// The Operator's Voiceprint and what kind of evidence it is.
///
/// The owned form of [`OperatorPrint`], which borrows. A caller reads this
/// once, holds it, and asks it for the borrowed view — and for what to
/// withhold from the general resolve, because those two answers have to
/// agree and used to be computed separately at the call site.
#[derive(Debug, Clone)]
pub enum OperatorEvidence {
    /// The Operator enrolled, and the clip has been embedded in this space.
    Enrolled(SeedVoice),
    /// Whatever inference has built, if anything.
    Learned(Option<SeedVoice>),
}

impl OperatorEvidence {
    /// The borrowed view [`identify`] takes, given whether this Meeting
    /// clears [`match_gate_met`].
    ///
    /// The gate reaches only the learned half: see the module documentation
    /// for why an enrolment is not something a gate about inference applies
    /// to.
    pub fn print(&self, gate: bool) -> OperatorPrint<'_> {
        match self {
            Self::Enrolled(seed) => OperatorPrint::Enrolled(seed),
            Self::Learned(known) => {
                OperatorPrint::Learned(gate.then_some(known.as_ref()).flatten())
            }
        }
    }

    /// The Speaker to keep out of the general resolve, where one must be.
    ///
    /// Withholding is the other half of the gate: below it the Operator's
    /// Voiceprint is kept out of the *whole* resolve, or the general pass
    /// carries it among every other seed and matches on it anyway, which
    /// would leave the gate looking like a rule and behaving like a
    /// comment. An enrolment is never withheld — there is no gate above it
    /// to enforce.
    pub fn withheld(&self, gate: bool) -> Option<&str> {
        match self {
            Self::Enrolled(_) => None,
            Self::Learned(known) => (!gate)
                .then_some(known.as_ref())
                .flatten()
                .map(|seed| seed.speaker_id.as_str()),
        }
    }
}

/// The Operator's Voiceprint, for seeding — in the given embedding space,
/// since one from another space would match nothing, or worse, something.
///
/// Reports [`OperatorEvidence::Learned(None)`](OperatorEvidence::Learned)
/// when the Operator's Voiceprint was made by a different embedding, which
/// is the same answer as having none: [`identify`] then has only rules 1 and
/// 2, and the channel decides. That is the correct behaviour after a model
/// change and the reason this takes a model at all.
///
/// An enrolment is only `Enrolled` once its clip has been embedded in *this*
/// space, which is what [`crate::store::speakers::is_enrolled`] checks. A
/// model change leaves the row standing with nothing behind it until the
/// re-embed runs, and answering `Enrolled` in that window would take a
/// rule-0 decision with no vector to decide with.
pub fn known_operator(
    connection: &rusqlite::Connection,
    model: &str,
    model_version: &str,
) -> anyhow::Result<OperatorEvidence> {
    let Some(speaker) = crate::store::speakers::operator(connection)? else {
        return Ok(OperatorEvidence::Learned(None));
    };
    if !speaker.has_voiceprint {
        return Ok(OperatorEvidence::Learned(None));
    }
    let seed = crate::store::speakers::voiceprints(connection, model, model_version)?
        .into_iter()
        .find(|(id, _, _)| *id == speaker.id)
        .map(|(speaker_id, vector, confirmed)| SeedVoice {
            speaker_id,
            vector,
            confirmed,
            model: model.to_string(),
            model_version: model_version.to_string(),
        });
    match seed {
        Some(seed)
            if crate::store::speakers::is_enrolled(
                connection,
                &speaker.id,
                model,
                model_version,
            )? =>
        {
            Ok(OperatorEvidence::Enrolled(seed))
        }
        other => Ok(OperatorEvidence::Learned(other)),
    }
}

/// Records the Operator's enrolment and rebuilds their Voiceprint from it.
///
/// The orchestration rather than the storage: [`crate::store::speakers::enrol`]
/// writes the rows, and the recomputation that turns them into a Voiceprint
/// lives in [`super::cluster`], so the one call that has to do both belongs
/// here beside the rules that read the result.
///
/// Returns the Operator's Speaker id, minting the row if this History has
/// never had one — enrolling is exactly the moment a History with no
/// Operator acquires one.
pub fn enrol(
    connection: &rusqlite::Connection,
    spans: &[crate::store::speakers::EnrolmentSpan],
    model: &str,
    model_version: &str,
    audio_path: &str,
    duration_ms: i64,
) -> anyhow::Result<String> {
    let id = ensure_operator_speaker(connection)?;
    crate::store::speakers::enrol(
        connection,
        &id,
        spans,
        model,
        model_version,
        audio_path,
        duration_ms,
    )?;
    super::cluster::refresh_voiceprint(connection, &id)?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diarize::Turn;
    use crate::diarize::fixture::FixtureDiarizer;
    use crate::diarize::{Cancel, Diarizer, MeetingAudio};

    fn run(mut diarizer: FixtureDiarizer) -> Diarization {
        let silence = vec![0.0_f32; 16_000];
        let audio = MeetingAudio {
            mic: &silence,
            system: &silence,
            sample_rate: 16_000,
        };
        diarizer
            .diarize(audio, &mut |_| {}, &Cancel::new())
            .expect("runs")
    }

    fn diarization(turns: Vec<Turn>) -> Diarization {
        Diarization {
            turns,
            embeddings: BTreeMap::new(),
        }
    }

    /// No fact either way, which is what every Meeting recorded before
    /// migration 14 carries.
    fn unsaid() -> MeetingFacts {
        MeetingFacts::default()
    }

    #[test]
    fn the_solo_operator_is_identified_from_the_channel_alone() {
        // The case the channel prior is genuinely strong for, and the one
        // most Operators are in most of the time.
        let d = run(FixtureDiarizer::solo());
        assert_eq!(
            identify(&d, OperatorPrint::Learned(None), &unsaid()),
            Identified::Dominant(Cluster(0))
        );
    }

    #[test]
    fn a_ten_second_solo_recording_enrolls_nobody() {
        // 100% of the mic channel, and still not a person worth keeping
        // forever: this is a microphone test, not a Meeting. The share says
        // the voice is alone, [`MIN_OPERATOR_MS`] says there was enough of
        // it — and `clean_two_speaker`, which used to pass this test with
        // 5.8 seconds, is on the wrong side of the new floor by design.
        let d = run(FixtureDiarizer::clean_two_speaker());
        assert_eq!(
            identify(&d, OperatorPrint::Learned(None), &unsaid()),
            Identified::Nobody
        );

        let d = diarization(vec![Turn::new(AudioChannel::Mic, 0, 10_000, 0)]);
        assert_eq!(
            identify(&d, OperatorPrint::Learned(None), &unsaid()),
            Identified::Nobody
        );

        // And the floor is a floor, not a ban: the same recording, long
        // enough, does enroll.
        let d = diarization(vec![Turn::new(AudioChannel::Mic, 0, MIN_OPERATOR_MS, 0)]);
        assert_eq!(
            identify(&d, OperatorPrint::Learned(None), &unsaid()),
            Identified::Dominant(Cluster(0))
        );
    }

    #[test]
    fn an_isolated_microphone_identifies_the_operator_with_no_act() {
        // Headphones were the only playing output and the microphone was
        // never swapped, so nothing but the room could have reached it.
        // There is no inference here for a threshold to get wrong — and no
        // floor either, because the fact is about the recording and not
        // about how much anyone said.
        let d = run(FixtureDiarizer::clean_two_speaker());
        let facts = MeetingFacts {
            mic_isolated: Some(true),
        };
        assert_eq!(
            identify(&d, OperatorPrint::Learned(None), &facts),
            Identified::IsolatedMic(vec![Cluster(0)]),
            "5.8 seconds is under the dominance floor and rule 1 does not care"
        );
    }

    #[test]
    fn a_second_voice_on_an_isolated_microphone_refuses_rule_one() {
        // The fact is still true: the far end could not reach that
        // microphone. It has stopped being sufficient, because it now names
        // two voices and only one of them is the person holding the laptop.
        // Enrolling both as "You" would be confidently wrong about somebody
        // else, with no act to correct it — so rule 1 declines and the
        // thresholded rule gets its turn.
        let d = run(FixtureDiarizer::shared_room());
        let facts = MeetingFacts {
            mic_isolated: Some(true),
        };
        assert_eq!(
            identify(&d, OperatorPrint::Learned(None), &facts),
            Identified::Nobody,
            "6.5s against 7.3s clears neither the dominance floor nor the margin, \
             so rule 2 refuses the colleague rather than guessing between them"
        );

        // And the refusal is rule 1's alone. The same two voices, with one
        // of them dominant enough for rule 2, still name that one — the
        // narrowing withholds the no-act enrolment, it does not withhold the
        // Operator.
        let d = diarization(vec![
            Turn::new(AudioChannel::Mic, 0, MIN_OPERATOR_MS, 0),
            Turn::new(
                AudioChannel::Mic,
                MIN_OPERATOR_MS + 500,
                MIN_OPERATOR_MS + 2_000,
                1,
            ),
        ]);
        assert_eq!(
            identify(&d, OperatorPrint::Learned(None), &facts),
            Identified::Dominant(Cluster(0))
        );
    }

    #[test]
    fn an_isolated_microphone_with_nobody_on_it_still_identifies_nobody() {
        // The Operator listened and said nothing. The fact is true and
        // names no one, which must fall through rather than produce an
        // empty Operator.
        let d = diarization(vec![Turn::new(AudioChannel::System, 0, 30_000, 0)]);
        assert_eq!(
            identify(
                &d,
                OperatorPrint::Learned(None),
                &MeetingFacts {
                    mic_isolated: Some(true)
                }
            ),
            Identified::Nobody
        );
    }

    #[test]
    fn a_shared_room_produces_no_bootstrap_rather_than_a_wrong_one() {
        // ADR-0029's amendment, and the reason it was made. Two real voices
        // on the mic channel, roughly balanced: the machine cannot know
        // which of them owns the laptop, and naming one gives a colleague's
        // words the Operator's name — invisibly, because the transcript
        // still reads perfectly plausibly.
        let d = run(FixtureDiarizer::shared_room());
        assert_eq!(
            identify(&d, OperatorPrint::Learned(None), &unsaid()),
            Identified::Nobody,
            "unidentified is the honest answer here"
        );
    }

    #[test]
    fn a_voiceprint_finds_the_operator_even_when_they_barely_speak() {
        // The shared room, once the Operator has been identified before.
        // Loudness would name the colleague; the Voiceprint does not.
        //
        // Stretched past the match gate, because a Meeting this short is
        // one where rule 3 is inadmissible — see the gate tests below.
        let mut diarizer = FixtureDiarizer::new(vec![
            Turn::new(AudioChannel::Mic, 0, 12_000, 0),
            Turn::new(AudioChannel::Mic, 13_500, 27_000, 1),
            Turn::new(AudioChannel::System, 28_500, 42_000, 2),
            Turn::new(AudioChannel::Mic, 43_500, 51_000, 0),
            Turn::new(AudioChannel::Mic, 51_600, 60_000, 1),
        ]);
        let mut d = diarizer
            .diarize(
                MeetingAudio {
                    mic: &[0.0; 16_000],
                    system: &[0.0; 16_000],
                    sample_rate: 16_000,
                },
                &mut |_| {},
                &Cancel::new(),
            )
            .expect("runs");
        assert!(match_gate_met(&d), "the fixture must clear its own gate");
        // Cluster 1 does most of the talking in that timeline's mic channel;
        // the Operator is cluster 0.
        let operator_vector = d.embeddings[&Cluster(0)].vector.clone();
        d.embeddings.insert(
            Cluster(0),
            Embedding::new(operator_vector.clone(), "fixture", "1", 10_000),
        );

        let known = SeedVoice {
            speaker_id: "me".into(),
            vector: operator_vector,
            confirmed: true,
            model: "fixture".into(),
            model_version: "1".into(),
        };
        assert_eq!(
            identify(&d, OperatorPrint::Learned(Some(&known)), &unsaid()),
            Identified::Recognized(Cluster(0))
        );
    }

    /// One voice with an embedding on the mic channel, and a `SeedVoice`
    /// that either matches it or does not. Enough to exercise rule 0, which
    /// asks one question of one vector.
    fn one_mic_voice(vector: Vec<f32>) -> Diarization {
        let mut d = diarization(vec![Turn::new(AudioChannel::Mic, 0, 4_000, 0)]);
        d.embeddings
            .insert(Cluster(0), Embedding::new(vector, "fixture", "1", 4_000));
        d
    }

    fn seed_of(vector: Vec<f32>) -> SeedVoice {
        SeedVoice {
            speaker_id: "you".into(),
            vector,
            confirmed: true,
            model: "fixture".into(),
            model_version: "1".into(),
        }
    }

    /// The whole point of rule 0: a Meeting the Operator does not dominate,
    /// where the channel would have named somebody else or nobody, and the
    /// enrolment names the right voice regardless.
    #[test]
    fn an_enrolment_names_its_voice_where_the_channel_would_not() {
        let mut d = one_mic_voice(vec![1.0, 0.0]);
        // A second, louder voice on the same microphone — the shared-room
        // case rule 1 refuses and rule 2 cannot resolve.
        d.turns.push(Turn::new(AudioChannel::Mic, 4_000, 60_000, 1));
        d.embeddings.insert(
            Cluster(1),
            Embedding::new(vec![0.0, 1.0], "fixture", "1", 56_000),
        );
        assert_eq!(
            identify(&d, OperatorPrint::Learned(None), &unsaid()),
            Identified::Dominant(Cluster(1)),
            "without an enrolment the loud voice takes the name"
        );

        let enrolled = seed_of(vec![1.0, 0.0]);
        assert_eq!(
            identify(&d, OperatorPrint::Enrolled(&enrolled), &unsaid()),
            Identified::Enrolled(Cluster(0)),
            "the enrolment names the voice that recorded it, not the loud one"
        );
    }

    /// A miss is `Nobody`, and specifically not a fall-through.
    ///
    /// The fall-through is the tempting version and the wrong one: the rules
    /// below rule 0 are the guess the enrolment was recorded to replace, so
    /// reaching them on a miss would put the guess back in exactly the
    /// Meetings where the trusted answer said no. Asserted against a
    /// diarization dominance *would* resolve, so a regression shows up as a
    /// name rather than as nothing.
    #[test]
    fn an_enrolment_that_matches_nothing_does_not_fall_through() {
        let d = one_mic_voice(vec![0.0, 1.0]);
        let mut d = d;
        d.turns.clear();
        d.turns
            .push(Turn::new(AudioChannel::Mic, 0, MIN_OPERATOR_MS + 5_000, 0));
        assert_eq!(
            identify(&d, OperatorPrint::Learned(None), &unsaid()),
            Identified::Dominant(Cluster(0)),
            "dominance would name this voice"
        );

        let elsewhere = seed_of(vec![1.0, 0.0]);
        assert_eq!(
            identify(&d, OperatorPrint::Enrolled(&elsewhere), &unsaid()),
            Identified::Nobody,
            "the Operator did not speak here, and the channel does not get a vote"
        );
    }

    /// Rule 3 is withheld below [`match_gate_met`]; rule 0 is not. A solo
    /// Meeting shorter than the gate is the case that would otherwise come
    /// back with no Operator in it at all.
    #[test]
    fn an_enrolment_decides_in_a_meeting_too_small_for_rule_three() {
        let d = one_mic_voice(vec![1.0, 0.0]);
        assert!(
            !match_gate_met(&d),
            "four seconds and one voice is under the gate"
        );

        let enrolled = seed_of(vec![1.0, 0.0]);
        assert_eq!(
            identify(&d, OperatorPrint::Enrolled(&enrolled), &unsaid()),
            Identified::Enrolled(Cluster(0))
        );
    }

    /// Rule 1's fact is still true and no longer sufficient. An isolated
    /// microphone says the far end could not reach it; it does not say the
    /// one voice on it is the person who enrolled.
    #[test]
    fn an_enrolment_outranks_an_isolated_microphone() {
        let d = one_mic_voice(vec![0.0, 1.0]);
        let facts = MeetingFacts {
            mic_isolated: Some(true),
        };
        assert_eq!(
            identify(&d, OperatorPrint::Learned(None), &facts),
            Identified::IsolatedMic(vec![Cluster(0)]),
            "rule 1 takes it when there is no enrolment"
        );

        let elsewhere = seed_of(vec![1.0, 0.0]);
        assert_eq!(
            identify(&d, OperatorPrint::Enrolled(&elsewhere), &facts),
            Identified::Nobody,
            "a colleague alone in the room on an isolated mic is not the Operator"
        );
    }

    #[test]
    fn the_match_gate_needs_both_enough_speech_and_more_than_one_voice() {
        // Thirty seconds and two voices, because each alone lets the match
        // fire where it says nothing: a long monologue is the dominance
        // rule's case, and a short exchange is too little evidence for a
        // cross-Meeting claim.
        let brief = diarization(vec![
            Turn::new(AudioChannel::Mic, 0, 5_000, 0),
            Turn::new(AudioChannel::System, 5_000, 12_000, 1),
        ]);
        assert!(!match_gate_met(&brief), "twelve seconds is under the gate");

        let monologue = diarization(vec![Turn::new(AudioChannel::Mic, 0, 600_000, 0)]);
        assert!(
            !match_gate_met(&monologue),
            "one voice is under the gate however long it talks"
        );

        let meeting = diarization(vec![
            Turn::new(AudioChannel::Mic, 0, 20_000, 0),
            Turn::new(AudioChannel::System, 20_000, 40_000, 1),
        ]);
        assert!(match_gate_met(&meeting));
    }

    #[test]
    fn a_voice_only_on_the_system_channel_is_never_the_operator() {
        // The far end is not in the room. Matching the Operator's Voiceprint
        // against system-channel audio would let their own echo — or a
        // recording of them played back — be identified as them.
        let d = Diarization {
            turns: vec![
                Turn::new(AudioChannel::System, 0, 30_000, 7),
                Turn::new(AudioChannel::System, 30_000, 45_000, 8),
            ],
            embeddings: [(
                Cluster(7),
                Embedding::new(vec![1.0, 0.0], "fixture", "1", 10_000),
            )]
            .into_iter()
            .collect(),
        };
        let known = SeedVoice {
            speaker_id: "me".into(),
            vector: vec![1.0, 0.0],
            confirmed: true,
            model: "fixture".into(),
            model_version: "1".into(),
        };
        assert!(
            match_gate_met(&d),
            "the match is admissible; the voice is not"
        );
        assert_eq!(
            identify(&d, OperatorPrint::Learned(Some(&known)), &unsaid()),
            Identified::Nobody
        );
    }

    #[test]
    fn a_silent_mic_channel_identifies_nobody() {
        // A meeting the Operator only listened to. Real, and it must not
        // produce a division by zero or a fabricated "You".
        let d = diarization(vec![Turn::new(AudioChannel::System, 0, 10_000, 0)]);
        assert_eq!(
            identify(&d, OperatorPrint::Learned(None), &unsaid()),
            Identified::Nobody
        );
    }

    #[test]
    fn one_quiet_voice_among_many_does_not_become_the_operator() {
        // Clearing the dominance share is not enough on its own: several
        // very quiet voices can leave a moderate one looking dominant
        // without it having any real claim. Sized past [`MIN_OPERATOR_MS`]
        // so it is the share refusing and not the floor.
        let d = diarization(vec![
            Turn::new(AudioChannel::Mic, 0, 25_000, 0),
            Turn::new(AudioChannel::Mic, 25_000, 32_000, 1),
            Turn::new(AudioChannel::Mic, 32_000, 39_000, 2),
            Turn::new(AudioChannel::Mic, 39_000, 46_000, 3),
        ]);
        assert_eq!(
            identify(&d, OperatorPrint::Learned(None), &unsaid()),
            Identified::Nobody
        );
    }

    #[test]
    fn echo_contaminated_audio_does_not_invent_a_room_mate() {
        // ADR-0029 requires this case. AEC runs from M1, so far-end voices
        // should not reach the mic channel at all — but a broken AEC
        // presents exactly as a second person in the room, and that is a
        // failure this milestone must not silently absorb into a Speaker.
        //
        // The assertion is that it stays *visible*: a second mic voice
        // blocks the bootstrap rather than being averaged into "You".
        let d = diarization(vec![
            Turn::new(AudioChannel::Mic, 0, 25_000, 0),
            // The far end, leaking through.
            Turn::new(AudioChannel::Mic, 25_000, 45_000, 1),
            Turn::new(AudioChannel::System, 25_000, 45_000, 2),
        ]);
        assert_eq!(
            identify(&d, OperatorPrint::Learned(None), &unsaid()),
            Identified::Nobody,
            "a phantom room-mate must not be quietly folded into You"
        );
    }

    #[test]
    fn the_operators_speaker_is_an_ordinary_row_in_the_registry() {
        // "You" is a display name, not a magic record. It has to be
        // renameable and its Voiceprint deletable like any other, because
        // ADR-0008's Registry promises a complete inventory with no
        // exceptions.
        let mut connection = rusqlite::Connection::open_in_memory().expect("open");
        crate::store::schema::migrate(&mut connection).expect("migrate");

        let id = ensure_operator_speaker(&connection).expect("create");
        assert_eq!(
            ensure_operator_speaker(&connection).expect("again"),
            id,
            "there is exactly one Operator"
        );

        let listed = crate::store::speakers::list(&connection).expect("list");
        assert!(
            listed
                .iter()
                .any(|speaker| speaker.id == id && speaker.is_operator)
        );

        crate::store::speakers::rename(&connection, &id, "Frank").expect("rename");
        assert_eq!(
            crate::store::speakers::get(&connection, &id)
                .expect("get")
                .expect("exists")
                .display_name
                .as_deref(),
            Some("Frank")
        );
        assert!(crate::store::speakers::delete_voiceprint(&connection, &id).expect("delete"));
    }

    #[test]
    fn a_second_you_is_impossible_rather_than_unlikely() {
        // The defect this ticket closes: the flag had no uniqueness
        // constraint, so a run that set it on a freshly minted row left two
        // flagged Speakers and a lookup that kept returning the older one.
        //
        // The schema refuses it now, which matters more than any call site
        // remembering to clear the old flag — a constraint holds for code
        // nobody has written yet.
        let mut connection = rusqlite::Connection::open_in_memory().expect("open");
        crate::store::schema::migrate(&mut connection).expect("migrate");

        let first = ensure_operator_speaker(&connection).expect("create");
        let second = crate::store::speakers::create(&connection, false).expect("create");

        let direct = connection.execute(
            "UPDATE speakers SET is_operator = 1 WHERE id = ?1",
            rusqlite::params![second.id],
        );
        assert!(
            direct.is_err(),
            "the schema must refuse a second flagged row, not merely discourage one"
        );

        // And the sanctioned way of moving the flag works: one Operator
        // before, one after, and it is the new row.
        crate::store::speakers::set_operator(&connection, &second.id).expect("move the flag");
        assert_eq!(
            crate::store::speakers::operator(&connection)
                .expect("lookup")
                .map(|speaker| speaker.id),
            Some(second.id)
        );
        assert!(
            !crate::store::speakers::get(&connection, &first)
                .expect("get")
                .expect("still there")
                .is_operator,
            "moving the flag clears the old one rather than adding a second"
        );
    }

    /// The learned seed, asserting the evidence is learned rather than
    /// enrolled. Every case below is inference, and a test that accepted
    /// either kind would not notice rule 0 firing where it should not.
    fn learned(evidence: &OperatorEvidence) -> Option<&SeedVoice> {
        match evidence {
            OperatorEvidence::Learned(seed) => seed.as_ref(),
            OperatorEvidence::Enrolled(_) => panic!("expected learned evidence, not an enrolment"),
        }
    }

    #[test]
    fn the_operator_is_not_offered_as_a_seed_until_they_have_a_voiceprint() {
        let mut connection = rusqlite::Connection::open_in_memory().expect("open");
        crate::store::schema::migrate(&mut connection).expect("migrate");
        assert!(learned(&known_operator(&connection, "m", "1").expect("none yet")).is_none());

        let id = ensure_operator_speaker(&connection).expect("create");
        assert!(learned(&known_operator(&connection, "m", "1").expect("still none")).is_none());

        crate::store::speakers::set_voiceprint(&connection, &id, &[1.0, 0.0], "m", "1")
            .expect("voiceprint");
        assert_eq!(
            learned(&known_operator(&connection, "m", "1").expect("now"))
                .map(|seed| seed.speaker_id.clone()),
            Some(id.clone())
        );
        // A Voiceprint from another front end is not the Operator's voice
        // as this model hears it.
        assert!(learned(&known_operator(&connection, "m", "2").expect("other space")).is_none());
    }

    /// A Voiceprint alone is inference; an enrolment is an act. The
    /// difference decides which rules run at all, so it is read from the
    /// record rather than assumed from the presence of a vector.
    #[test]
    fn an_enrolment_is_told_apart_from_a_voiceprint_inference_built() {
        let mut connection = rusqlite::Connection::open_in_memory().expect("open");
        crate::store::schema::migrate(&mut connection).expect("migrate");
        let id = ensure_operator_speaker(&connection).expect("create");
        crate::store::speakers::set_voiceprint(&connection, &id, &[1.0, 0.0], "m", "1")
            .expect("voiceprint");
        assert!(
            matches!(
                known_operator(&connection, "m", "1").expect("learned"),
                OperatorEvidence::Learned(Some(_))
            ),
            "a Voiceprint with no enrolment behind it is inference"
        );

        enrol(
            &connection,
            &[crate::store::speakers::EnrolmentSpan {
                vector: vec![1.0, 0.0],
                voiced_ms: 25_000,
                start_ms: 0,
                end_ms: 25_000,
            }],
            "m",
            "1",
            "audio/enrolment-you.mp3",
            25_000,
        )
        .expect("enrol");
        assert!(matches!(
            known_operator(&connection, "m", "1").expect("enrolled"),
            OperatorEvidence::Enrolled(_)
        ));

        // The model-change window: the wipe takes every vector and leaves
        // the enrolment row standing. Until the clip is re-embedded there is
        // nothing for rule 0 to decide with, and claiming otherwise would
        // name the Operator from an empty hand.
        assert!(
            matches!(
                known_operator(&connection, "m", "2").expect("other space"),
                OperatorEvidence::Learned(None)
            ),
            "an enrolment in another space is not evidence in this one"
        );
    }

    /// The gate is about how far an inference may be trusted, so it reaches
    /// the learned half and not the enrolled one — in both directions, since
    /// withholding and the rule have to agree about the same Meeting.
    #[test]
    fn the_match_gate_reaches_inference_and_not_an_enrolment() {
        let seed = SeedVoice {
            speaker_id: "you".to_string(),
            vector: vec![1.0, 0.0],
            confirmed: true,
            model: "m".to_string(),
            model_version: "1".to_string(),
        };

        let learned = OperatorEvidence::Learned(Some(seed.clone()));
        assert!(matches!(
            learned.print(true),
            OperatorPrint::Learned(Some(_))
        ));
        assert!(matches!(learned.print(false), OperatorPrint::Learned(None)));
        assert_eq!(learned.withheld(false), Some("you"));
        assert_eq!(learned.withheld(true), None);

        let enrolled = OperatorEvidence::Enrolled(seed);
        assert!(matches!(enrolled.print(true), OperatorPrint::Enrolled(_)));
        assert!(
            matches!(enrolled.print(false), OperatorPrint::Enrolled(_)),
            "an enrolment decides in a Meeting too small for rule 3"
        );
        assert_eq!(
            enrolled.withheld(false),
            None,
            "there is no gate above an enrolment to enforce by withholding"
        );
    }
}
