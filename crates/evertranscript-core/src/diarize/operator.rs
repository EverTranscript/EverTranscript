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
//! So the channel narrows the field and never decides it:
//!
//! - With a Voiceprint for the Operator, mic-channel clusters are matched
//!   against it by the same conservative rule everything else uses.
//! - Without one, the Operator is **bootstrapped from the dominant mic
//!   voice** — but only where one voice actually dominates. Two people
//!   sharing a microphone evenly produce no bootstrap at all, which is the
//!   honest outcome: the machine has no way to know which of them owns the
//!   laptop, and guessing gives someone else's words the Operator's name.

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
/// between is ambiguous, and this milestone would rather leave the Operator
/// unidentified for one Meeting than name the wrong person — an unnamed
/// Speaker is a visible gap, a wrongly-named one is invisible.
pub const DOMINANCE: f32 = 0.75;

/// How far the leading mic voice must beat the runner-up, as a share of mic
/// time. Guards the case where one voice clears [`DOMINANCE`] only because
/// the others are numerous rather than quiet.
pub const DOMINANCE_MARGIN: f32 = 0.5;

/// Which cluster is the Operator, if the evidence supports naming one.
///
/// `known` is the Operator's existing Voiceprint, when they have one.
pub fn identify(diarization: &Diarization, known: Option<&SeedVoice>) -> Option<Cluster> {
    // A Voiceprint is better evidence than loudness, and it is the only
    // thing that keeps "You" correct in a room where the Operator is not the
    // one doing most of the talking.
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
        if found.is_some() {
            return found;
        }
    }

    bootstrap(diarization)
}

/// The Operator's first Meeting, before any Voiceprint exists.
fn bootstrap(diarization: &Diarization) -> Option<Cluster> {
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

/// The Operator's Voiceprint, for seeding.
pub fn known_operator(connection: &rusqlite::Connection) -> anyhow::Result<Option<SeedVoice>> {
    let Some(speaker) = crate::store::speakers::operator(connection)? else {
        return Ok(None);
    };
    if !speaker.has_voiceprint {
        return Ok(None);
    }
    Ok(crate::store::speakers::voiceprints(connection)?
        .into_iter()
        .find(|(id, _, _)| *id == speaker.id)
        .map(|(speaker_id, vector, confirmed)| SeedVoice {
            speaker_id,
            vector,
            confirmed,
        }))
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

    #[test]
    fn the_solo_operator_is_identified_from_the_channel_alone() {
        // The case the channel prior is genuinely strong for, and the one
        // most Operators are in most of the time.
        let d = run(FixtureDiarizer::clean_two_speaker());
        assert_eq!(identify(&d, None), Some(Cluster(0)));
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
            identify(&d, None),
            None,
            "unidentified is the honest answer here"
        );
    }

    #[test]
    fn a_voiceprint_finds_the_operator_even_when_they_barely_speak() {
        // The shared room, once the Operator has been identified before.
        // Loudness would name the colleague; the Voiceprint does not.
        let mut d = run(FixtureDiarizer::shared_room());
        // Cluster 1 does most of the talking in that fixture's mic channel;
        // the Operator is cluster 0.
        let operator_vector = d.embeddings[&Cluster(0)].vector.clone();
        d.embeddings.insert(
            Cluster(0),
            Embedding::new(operator_vector.clone(), "fixture", "1"),
        );

        let known = SeedVoice {
            speaker_id: "me".into(),
            vector: operator_vector,
            confirmed: true,
        };
        assert_eq!(identify(&d, Some(&known)), Some(Cluster(0)));
    }

    #[test]
    fn a_voice_only_on_the_system_channel_is_never_the_operator() {
        // The far end is not in the room. Matching the Operator's Voiceprint
        // against system-channel audio would let their own echo — or a
        // recording of them played back — be identified as them.
        let d = Diarization {
            turns: vec![Turn::new(AudioChannel::System, 0, 10_000, 7)],
            embeddings: [(Cluster(7), Embedding::new(vec![1.0, 0.0], "fixture", "1"))]
                .into_iter()
                .collect(),
        };
        let known = SeedVoice {
            speaker_id: "me".into(),
            vector: vec![1.0, 0.0],
            confirmed: true,
        };
        assert_eq!(identify(&d, Some(&known)), None);
    }

    #[test]
    fn a_silent_mic_channel_identifies_nobody() {
        // A meeting the Operator only listened to. Real, and it must not
        // produce a division by zero or a fabricated "You".
        let d = diarization(vec![Turn::new(AudioChannel::System, 0, 10_000, 0)]);
        assert_eq!(identify(&d, None), None);
    }

    #[test]
    fn one_quiet_voice_among_many_does_not_become_the_operator() {
        // Clearing the dominance share is not enough on its own: several
        // very quiet voices can leave a moderate one looking dominant
        // without it having any real claim.
        let d = diarization(vec![
            Turn::new(AudioChannel::Mic, 0, 1_000, 0),
            Turn::new(AudioChannel::Mic, 1_000, 1_300, 1),
            Turn::new(AudioChannel::Mic, 1_300, 1_600, 2),
            Turn::new(AudioChannel::Mic, 1_600, 1_900, 3),
        ]);
        assert_eq!(identify(&d, None), None);
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
            Turn::new(AudioChannel::Mic, 0, 5_000, 0),
            // The far end, leaking through.
            Turn::new(AudioChannel::Mic, 5_000, 9_000, 1),
            Turn::new(AudioChannel::System, 5_000, 9_000, 2),
        ]);
        assert_eq!(
            identify(&d, None),
            None,
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
    fn the_operator_is_not_offered_as_a_seed_until_they_have_a_voiceprint() {
        let mut connection = rusqlite::Connection::open_in_memory().expect("open");
        crate::store::schema::migrate(&mut connection).expect("migrate");
        assert!(known_operator(&connection).expect("none yet").is_none());

        let id = ensure_operator_speaker(&connection).expect("create");
        assert!(known_operator(&connection).expect("still none").is_none());

        crate::store::speakers::set_voiceprint(&connection, &id, &[1.0, 0.0], "m", "1")
            .expect("voiceprint");
        assert_eq!(
            known_operator(&connection)
                .expect("now")
                .map(|seed| seed.speaker_id),
            Some(id)
        );
    }
}
