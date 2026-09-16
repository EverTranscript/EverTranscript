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
//! So the channel narrows the field and never decides it. Three rules do,
//! in this order (ADR-0029 as amended):
//!
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
/// differ in how much they are trusted downstream and in how many voices
/// they can name at once, and a caller that only got `Option<Cluster>` back
/// could not tell the isolated-mic case — where every mic voice is the same
/// person — from a match on one of several.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identified {
    /// Rule 1. Nothing but the room reached the microphone, so every voice
    /// on it is the Operator, confirmed without any act.
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
            Identified::Dominant(cluster) | Identified::Recognized(cluster) => {
                std::slice::from_ref(cluster)
            }
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

/// Which clusters are the Operator, by the three rules in order.
///
/// `known` is the Operator's existing Voiceprint, and the caller must have
/// already withheld it when [`match_gate_met`] is false: passing it here in
/// a Meeting under the gate would let rule 3 fire on evidence the rule says
/// is inadmissible, and — worse — it would mean the general resolve had
/// carried that Voiceprint among all its seeds and matched on it anyway,
/// which is what would make the gate decorative.
pub fn identify(
    diarization: &Diarization,
    known: Option<&SeedVoice>,
    facts: &MeetingFacts,
) -> Identified {
    // 1. The microphone heard the room and nothing else. There is no
    //    inference to make and nothing for a threshold to get wrong.
    if facts.mic_isolated == Some(true) {
        let mut clusters: Vec<Cluster> = diarization
            .turns
            .iter()
            .filter(|turn| turn.channel == AudioChannel::Mic)
            .map(|turn| turn.cluster)
            .collect();
        clusters.sort_unstable();
        clusters.dedup();
        if !clusters.is_empty() {
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
    if let Some(known) = known {
        let mic_clusters: BTreeMap<Cluster, Embedding> = diarization
            .embeddings
            .iter()
            .filter(|(cluster, _)| speaks_on_mic(diarization, **cluster))
            .map(|(cluster, embedding)| (*cluster, embedding.clone()))
            .collect();
        let matched = resolve(&mic_clusters, std::slice::from_ref(known));
        let found = matched.into_iter().find_map(|(cluster, outcome)| {
            matches!(outcome, Resolved::Existing(ref id) if *id == known.speaker_id)
                .then_some(cluster)
        });
        if let Some(cluster) = found {
            return Identified::Recognized(cluster);
        }
    }

    Identified::Nobody
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

/// The Operator's Voiceprint, for seeding — in the given embedding space,
/// since one from another space would match nothing, or worse, something.
///
/// Returns `None` when the Operator's Voiceprint was made by a different
/// embedding, which is the same answer as having none: [`identify`] then has
/// only rules 1 and 2, and the channel decides. That is the correct
/// behaviour after a model change and the reason this takes a model at all
pub fn known_operator(
    connection: &rusqlite::Connection,
    model: &str,
    model_version: &str,
) -> anyhow::Result<Option<SeedVoice>> {
    let Some(speaker) = crate::store::speakers::operator(connection)? else {
        return Ok(None);
    };
    if !speaker.has_voiceprint {
        return Ok(None);
    }
    Ok(
        crate::store::speakers::voiceprints(connection, model, model_version)?
            .into_iter()
            .find(|(id, _, _)| *id == speaker.id)
            .map(|(speaker_id, vector, confirmed)| SeedVoice {
                speaker_id,
                vector,
                confirmed,
            }),
    )
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
            identify(&d, None, &unsaid()),
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
        assert_eq!(identify(&d, None, &unsaid()), Identified::Nobody);

        let d = diarization(vec![Turn::new(AudioChannel::Mic, 0, 10_000, 0)]);
        assert_eq!(identify(&d, None, &unsaid()), Identified::Nobody);

        // And the floor is a floor, not a ban: the same recording, long
        // enough, does enroll.
        let d = diarization(vec![Turn::new(AudioChannel::Mic, 0, MIN_OPERATOR_MS, 0)]);
        assert_eq!(
            identify(&d, None, &unsaid()),
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
            identify(&d, None, &facts),
            Identified::IsolatedMic(vec![Cluster(0)]),
            "5.8 seconds is under the dominance floor and rule 1 does not care"
        );

        // Two voices in a room the far end could not reach are both the
        // Operator's microphone — which is the one case where "You" names
        // more than one cluster.
        let d = run(FixtureDiarizer::shared_room());
        assert_eq!(
            identify(&d, None, &facts),
            Identified::IsolatedMic(vec![Cluster(0), Cluster(1)])
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
                None,
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
            identify(&d, None, &unsaid()),
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
        };
        assert_eq!(
            identify(&d, Some(&known), &unsaid()),
            Identified::Recognized(Cluster(0))
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
        };
        assert!(match_gate_met(&d), "the match is admissible; the voice is not");
        assert_eq!(identify(&d, Some(&known), &unsaid()), Identified::Nobody);
    }

    #[test]
    fn a_silent_mic_channel_identifies_nobody() {
        // A meeting the Operator only listened to. Real, and it must not
        // produce a division by zero or a fabricated "You".
        let d = diarization(vec![Turn::new(AudioChannel::System, 0, 10_000, 0)]);
        assert_eq!(identify(&d, None, &unsaid()), Identified::Nobody);
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
        assert_eq!(identify(&d, None, &unsaid()), Identified::Nobody);
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
            identify(&d, None, &unsaid()),
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

    #[test]
    fn the_operator_is_not_offered_as_a_seed_until_they_have_a_voiceprint() {
        let mut connection = rusqlite::Connection::open_in_memory().expect("open");
        crate::store::schema::migrate(&mut connection).expect("migrate");
        assert!(
            known_operator(&connection, "m", "1")
                .expect("none yet")
                .is_none()
        );

        let id = ensure_operator_speaker(&connection).expect("create");
        assert!(
            known_operator(&connection, "m", "1")
                .expect("still none")
                .is_none()
        );

        crate::store::speakers::set_voiceprint(&connection, &id, &[1.0, 0.0], "m", "1")
            .expect("voiceprint");
        assert_eq!(
            known_operator(&connection, "m", "1")
                .expect("now")
                .map(|seed| seed.speaker_id),
            Some(id.clone())
        );
        // A Voiceprint from another front end is not the Operator's voice
        // as this model hears it.
        assert!(
            known_operator(&connection, "m", "2")
                .expect("other space")
                .is_none()
        );
    }
}
