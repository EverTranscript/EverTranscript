//! Re-teaching one Meeting's named voices in a new embedding space,
//! **written and unreachable from production**.
//!
//! Ticket 12's bounded rebuild. A model change clears every Voiceprint; this
//! is what earns the named ones back, one Meeting at a time, from the audio
//! that is still on disk rather than from anything the old model concluded.
//!
//! # What it re-embeds, and what it refuses to
//!
//! **The Operator's ranges, not the machine's clusters.** Each stretch of
//! audio a segment covers is re-embedded on its own, and it becomes evidence
//! for whoever that segment is attributed to *now* — the newest correction if
//! there is one, the machine's conclusion if there is not. The other half of
//! a correction is kept: a segment a correction took away from somebody is
//! negative evidence against them, which is what stops the same wrong match
//! happening again (ADR-0009 as amended).
//!
//! Three things are deliberately not sources:
//!
//! * **cluster centroids.** [`super::cluster::claims`] says plainly why: a
//!   cluster's vector is built before reconciliation and can carry speech no
//!   segment covers at all. Unanimity among segments cannot speak for the
//!   rest of what went into it. Claims is a shortcut for assigning a whole
//!   cluster, not a licence to enrol its vector — and not a gate on this
//!   either, because a named Speaker's own usable ranges are still theirs
//!   when the cluster around them comes out mixed.
//! * **the old model's saved sample cuts.** ADR-0037 rejected re-embedding
//!   those; the ranges come from the transcript, which is in the record.
//! * **pseudonyms, forgotten Speakers and the Operator.** The first are
//!   re-minted and renumbered rather than relearned, the second is the whole
//!   point of the mark (ADR-0009), and the Operator is rebuilt by the three
//!   channel rules alone (ADR-0029 as amended) — never from the previous
//!   model's guesses about which voice was theirs.
//!
//! # The scope a replacement owns
//!
//! **Every exemplar an eligible Speaker has from this Meeting**, whichever
//! writer produced it. Not a narrower scope tagged for this one: the sibling
//! writer [`crate::store::speakers::correct_segment`] also writes rows
//! against this Meeting, and a correction that moved a segment away and then
//! back has already left a negative behind. Replacing only rows this path
//! wrote would re-derive the positive and leave that negative standing, so
//! the Speaker would carry evidence for and against the same audio. The
//! corrections themselves are untouched — they are in `attribution_hints`,
//! which is the record; an exemplar is derived from them, and this derives
//! it again.
//!
//! That scope is `(speaker_id, meeting_id)`, and both columns already exist.
//! No new provenance is needed to tell the writers apart, because the
//! replacement does not tell them apart: it replaces what they jointly say
//! about one Meeting with what the Meeting now says.
//!
//! # Nothing calls this
//!
//! It is reachable from tests and from nothing else. Activation is the
//! user's model decision plus ticket 05, the same two this whole ticket
//! waits on.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Result;
use evertranscript_protocol::AudioChannel;
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use rusqlite::params;

use crate::store::speakers::{self, NewExemplar, Sample};

/// One stretch of one Meeting's audio, and what it is evidence of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Range {
    /// The segment it came from. Carried so a re-read can tell the same
    /// range apart from a coincidentally identical one.
    pub segment_id: String,
    pub speaker_id: String,
    pub sample: Sample,
    /// True when a correction took this audio *away* from `speaker_id`.
    pub is_negative: bool,
    /// True when a correction is what put this range where it is, which
    /// makes it the strongest evidence the system has about that voice.
    pub from_operator: bool,
}

/// What one Meeting is about to say, and whose evidence it replaces.
///
/// Read whole before anything is embedded, and read again inside the
/// transaction that writes: embedding is slow and unlocked, and a
/// correction, a deletion or a forgetting during it must not be overwritten
/// by vectors computed from the world as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub meeting_id: String,
    /// The Meeting's kept audio, relative to the History folder.
    pub audio_path: String,
    pub ranges: Vec<Range>,
    /// Every eligible Speaker whose evidence from this Meeting this
    /// replaces — including one that ends up with none. A Speaker whose
    /// every segment was corrected away keeps no rows here, and its
    /// Voiceprint has to be recomputed *because* they are gone.
    pub owners: BTreeSet<String>,
}

/// Reads what this Meeting has to teach, without embedding anything.
///
/// `None` when there is no audio to read: a Meeting whose recording was
/// never kept or has been deleted keeps the attributions it has (ADR-0035).
pub fn plan(connection: &Connection, meeting_id: &str) -> Result<Option<Plan>> {
    let audio_path: Option<String> = connection
        .query_row(
            "SELECT audio_path FROM meetings WHERE id = ?1",
            params![meeting_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let Some(audio_path) = audio_path else {
        return Ok(None);
    };

    // Named, not forgotten, not the Operator — the same set `claims` works
    // from, and for the same reasons. A pseudonym is absent because it has no
    // name, so nothing here has to exclude it by hand.
    let operator = speakers::operator(connection)?.map(|speaker| speaker.id);
    let eligible: BTreeSet<String> = speakers::relearnable(connection)?
        .into_iter()
        .map(|speaker| speaker.id)
        .filter(|id| Some(id) != operator.as_ref())
        .collect();

    let segments: Vec<(String, String, i64, i64)> = {
        let mut statement = connection.prepare(
            "SELECT id, channel, start_ms, end_ms FROM transcript_segments \
              WHERE meeting_id = ?1 ORDER BY sequence",
        )?;
        let rows = statement.query_map(params![meeting_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
        rows.collect::<rusqlite::Result<_>>()?
    };

    let mut ranges = Vec::new();
    for (segment_id, channel, start_ms, end_ms) in segments {
        let Some(channel) = AudioChannel::parse(&channel) else {
            continue;
        };
        // An empty or backwards span is not audio; the reader would return
        // nothing for it and the model could not embed it anyway.
        if end_ms <= start_ms {
            continue;
        }
        let sample = Sample {
            channel,
            start_ms,
            end_ms,
        };
        let corrected = speakers::attributed_speaker(connection, &segment_id)?.is_some()
            && speakers::replaced_speaker(connection, &segment_id)?.is_some();

        // Who owns this audio now. The newest correction if there is one,
        // the machine's conclusion otherwise.
        if let Some(owner) = speakers::attributed_speaker(connection, &segment_id)?
            .filter(|id| eligible.contains(id))
        {
            ranges.push(Range {
                segment_id: segment_id.clone(),
                speaker_id: owner,
                sample,
                is_negative: false,
                from_operator: corrected,
            });
        }
        // And who it was taken from. Always the Operator's act, so always
        // their evidence.
        if let Some(denied) = speakers::replaced_speaker(connection, &segment_id)?
            .filter(|id| eligible.contains(id))
            .filter(|id| {
                speakers::attributed_speaker(connection, &segment_id)
                    .ok()
                    .flatten()
                    .as_ref()
                    != Some(id)
            })
        {
            ranges.push(Range {
                segment_id,
                speaker_id: denied,
                sample,
                is_negative: true,
                from_operator: true,
            });
        }
    }

    // Whoever this Meeting will speak for, plus whoever it spoke for before
    // and no longer does — their rows go too, and their Voiceprint with them.
    let mut owners: BTreeSet<String> = ranges
        .iter()
        .map(|range| range.speaker_id.clone())
        .collect();
    {
        let mut statement = connection
            .prepare("SELECT DISTINCT speaker_id FROM speaker_exemplars WHERE meeting_id = ?1")?;
        let rows = statement.query_map(params![meeting_id], |row| row.get::<_, String>(0))?;
        for held in rows {
            let held = held?;
            if eligible.contains(&held) {
                owners.insert(held);
            }
        }
    }

    Ok(Some(Plan {
        meeting_id: meeting_id.to_string(),
        audio_path,
        ranges,
        owners,
    }))
}

/// Reading one stretch of one channel of a Meeting's kept audio.
///
/// A parameter rather than a direct call so the range arithmetic can be
/// checked without a decoder — and checked on what it *asks for*, which is
/// the property that matters: a re-embedding that reached outside the ranges
/// the Operator's attribution covers would be learning a voice from somebody
/// else's words. Production passes [`crate::audio::sample::read`].
pub type Read<'a> = dyn FnMut(&Path, AudioChannel, u64, u64) -> Result<(Vec<f32>, u32)> + 'a;

/// Embeds each planned range, in order. `None` where a range yielded
/// nothing — unreadable audio, or too little of it for the model.
///
/// The same reading and resampling [`super::runner::rebuild`] does, and the
/// model is a callback for the same reason.
pub fn embed_ranges(
    plan: &Plan,
    history_dir: &Path,
    read: &mut Read<'_>,
    embed: &mut super::runner::Embed<'_>,
) -> Vec<Option<Vec<f32>>> {
    let path = history_dir.join(&plan.audio_path);
    plan.ranges
        .iter()
        .map(|range| {
            let (samples, rate) = read(
                &path,
                range.sample.channel,
                range.sample.start_ms.max(0) as u64,
                range.sample.end_ms.max(0) as u64,
            )
            .inspect_err(|error| {
                tracing::warn!(%error, segment = %range.segment_id, "could not read the range to re-embed");
            })
            .ok()?;
            embed(&super::runner::resample_to_model_rate(&samples, rate))
                .inspect_err(|error| {
                    tracing::warn!(%error, segment = %range.segment_id, "could not re-embed the range");
                })
                .ok()
                .flatten()
        })
        .collect()
}

/// Why a commit wrote nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// The Meeting says something else now. A correction, a deletion, a
    /// rename or a forgetting landed while the vectors were being computed,
    /// and they describe a world that has moved.
    Moved,
    /// The Meeting's audio is gone, so there is nothing to plan from.
    Gone,
}

/// Replaces everything this Meeting says about its named voices, and
/// recomputes the Voiceprints that changed.
///
/// `vectors` is parallel to `plan.ranges`. Call inside a transaction: the
/// delete, the inserts and the recomputation are one change, and a failure
/// part-way through must leave the previous evidence whole rather than a
/// Speaker with half of two models' worth.
///
/// **Re-reads the plan first.** Embedding happens outside any lock, and the
/// record can move under it — a correction, a Meeting deleted, a Speaker
/// forgotten or renamed. Comparing the plan it re-reads against the one the
/// vectors were computed for catches every one of those at once, and is the
/// reason this cannot overwrite a newer correction or resurrect evidence for
/// a Speaker somebody has since forgotten.
pub fn commit(
    connection: &Connection,
    plan: &Plan,
    vectors: &[Option<Vec<f32>>],
    model: &str,
    model_version: &str,
) -> Result<std::result::Result<usize, Refused>> {
    let Some(current) = self::plan(connection, &plan.meeting_id)? else {
        return Ok(Err(Refused::Gone));
    };
    if current != *plan {
        return Ok(Err(Refused::Moved));
    }

    // Every exemplar these Speakers have from this Meeting, whoever wrote
    // it — see the module docs. Confined to this Meeting, so what they were
    // taught anywhere else stands.
    for speaker_id in &plan.owners {
        connection.execute(
            "DELETE FROM speaker_exemplars WHERE speaker_id = ?1 AND meeting_id = ?2",
            params![speaker_id, plan.meeting_id],
        )?;
    }

    let mut written = 0;
    for (range, vector) in plan.ranges.iter().zip(vectors) {
        let Some(vector) = vector else {
            continue;
        };
        speakers::add_exemplar(
            connection,
            NewExemplar {
                speaker_id: &range.speaker_id,
                meeting_id: Some(&plan.meeting_id),
                vector,
                model,
                model_version,
                voiced_ms: range.sample.end_ms - range.sample.start_ms,
                from_operator: range.from_operator,
                is_negative: range.is_negative,
                sample: Some(range.sample),
            },
        )?;
        written += 1;
    }

    // Including the ones that ended up with nothing: `seeds` reads the
    // Voiceprint column, not the rows, so a Speaker whose evidence went and
    // whose vector stayed would go on recognizing itself from a model that
    // no longer describes it.
    for speaker_id in &plan.owners {
        super::cluster::refresh_voiceprint(connection, speaker_id)?;
    }
    Ok(Ok(written))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::speakers::Exemplar;

    const RATE: u32 = crate::diarize::fbank::SAMPLE_RATE;

    /// A History with the current schema and one Meeting with kept audio.
    ///
    /// No file behind it: the reader is injected, so what these check is
    /// which stretches get asked for, which is the question. Whether
    /// symphonia can decode a given container is its own concern and is not
    /// re-tested here.
    fn history() -> Connection {
        let mut connection = Connection::open_in_memory().expect("open");
        crate::store::schema::configure(&connection).expect("configure");
        crate::store::schema::migrate(&mut connection).expect("migrate");
        connection
            .execute(
                "INSERT INTO meetings (id, started_at, audio_path, created_at, updated_at) \
                 VALUES ('m1', '2024-01-01T00:00:00Z', 'm1.wav', 'now', 'now')",
                [],
            )
            .expect("meeting");
        connection
    }

    fn speaker(connection: &Connection, id: &str, name: Option<&str>) {
        connection
            .execute(
                "INSERT INTO speakers (id, display_name, confirmed, created_at) \
                 VALUES (?1, ?2, 1, 'now')",
                params![id, name],
            )
            .expect("speaker");
    }

    fn segment(connection: &Connection, id: &str, sequence: i64, span: (i64, i64), owner: &str) {
        connection
            .execute(
                "INSERT INTO transcript_segments \
                    (id, meeting_id, sequence, channel, start_ms, end_ms, text, speaker_id) \
                 VALUES (?1, 'm1', ?2, 'mic', ?3, ?4, 'hello', ?5)",
                params![id, sequence, span.0, span.1, owner],
            )
            .expect("segment");
    }

    /// A correction, as the record holds one: an appended hint, never an
    /// overwrite (ADR-0009 as amended).
    fn correct(connection: &Connection, segment_id: &str, to: &str, from: Option<&str>) {
        connection
            .execute(
                "INSERT INTO attribution_hints \
                    (id, segment_id, speaker_id, replaced_speaker_id, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    uuid::Uuid::now_v7().to_string(),
                    segment_id,
                    to,
                    from,
                    crate::store::now_rfc3339()
                ],
            )
            .expect("hint");
    }

    /// A reader that records every stretch it is asked for and answers a
    /// second of silence, and an embedder that turns that into a vector
    /// naming the request. Between them they say exactly what audio the
    /// model was shown.
    fn reader(
        asked: &mut Vec<(AudioChannel, u64, u64)>,
    ) -> impl FnMut(&Path, AudioChannel, u64, u64) -> Result<(Vec<f32>, u32)> + '_ {
        move |_path, channel, start_ms, end_ms| {
            asked.push((channel, start_ms, end_ms));
            Ok((vec![start_ms as f32; RATE as usize], RATE))
        }
    }

    fn embedder(
        samples: &[f32],
    ) -> std::result::Result<Option<Vec<f32>>, crate::diarize::DiarizeError> {
        Ok(Some(vec![
            samples.first().copied().unwrap_or(0.0),
            samples.len() as f32,
        ]))
    }

    fn held(connection: &Connection, speaker_id: &str) -> Vec<Exemplar> {
        speakers::exemplars(connection, speaker_id).expect("exemplars")
    }

    #[test]
    fn the_model_is_handed_only_the_audio_the_ranges_cover_in_both_directions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let connection = history();
        speaker(&connection, "alice", Some("Alice"));
        speaker(&connection, "bob", Some("Bob"));
        segment(&connection, "s1", 1, (1000, 2000), "alice");
        // Taken from Bob and given to Alice: positive for one, negative for
        // the other, over the same stretch.
        segment(&connection, "s2", 2, (2500, 3000), "bob");
        correct(&connection, "s2", "alice", Some("bob"));

        let plan = self::plan(&connection, "m1").expect("plan").expect("audio");
        let mut asked = Vec::new();
        let vectors = embed_ranges(&plan, dir.path(), &mut reader(&mut asked), &mut embedder);

        assert_eq!(
            asked,
            vec![
                (AudioChannel::Mic, 1000, 2000),
                (AudioChannel::Mic, 2500, 3000),
                (AudioChannel::Mic, 2500, 3000),
            ],
            "only the segments' own audio, once for the positive and once for the negative"
        );
        assert_eq!(vectors.len(), 3);
        assert!(vectors.iter().all(Option::is_some));

        let signs: Vec<(&str, bool)> = plan
            .ranges
            .iter()
            .map(|range| (range.speaker_id.as_str(), range.is_negative))
            .collect();
        assert_eq!(
            signs,
            vec![("alice", false), ("alice", false), ("bob", true)],
            "the correction teaches Alice the voice and teaches Bob it is not his"
        );
    }

    /// The failure the sibling writer causes, and the reason the scope is the
    /// whole Meeting rather than rows this path tagged as its own.
    #[test]
    fn a_correction_taken_away_and_given_back_replaces_rather_than_accumulates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let connection = history();
        speaker(&connection, "alice", Some("Alice"));
        speaker(&connection, "bob", Some("Bob"));
        segment(&connection, "s1", 1, (1000, 2000), "alice");

        // What `correct_segment` leaves behind: away from Alice, then back.
        speakers::add_exemplar(
            &connection,
            NewExemplar {
                speaker_id: "alice",
                meeting_id: Some("m1"),
                vector: &[9.0, 9.0],
                model: "old",
                model_version: "1",
                voiced_ms: 1000,
                from_operator: true,
                is_negative: true,
                sample: None,
            },
        )
        .expect("stale negative");
        correct(&connection, "s1", "bob", Some("alice"));
        correct(&connection, "s1", "alice", Some("bob"));

        let plan = self::plan(&connection, "m1").expect("plan").expect("audio");
        let mut asked = Vec::new();
        let vectors = embed_ranges(&plan, dir.path(), &mut reader(&mut asked), &mut embedder);
        let written = commit(&connection, &plan, &vectors, "new", "2")
            .expect("commit")
            .expect("accepted");
        assert_eq!(
            written, 2,
            "the latest correction says Alice, and says it was not Bob"
        );

        let alice = held(&connection, "alice");
        assert_eq!(alice.len(), 1, "the stale negative did not survive");
        assert!(
            !alice[0].is_negative,
            "and what is left is the positive the latest correction implies"
        );
        assert_eq!(alice[0].model, "new");
        let bob = held(&connection, "bob");
        assert_eq!(bob.len(), 1);
        assert!(bob[0].is_negative, "the other half of the same correction");

        // And running it again does not stack a second copy.
        let again = self::plan(&connection, "m1").expect("plan").expect("audio");
        let mut twice = Vec::new();
        let vectors = embed_ranges(&again, dir.path(), &mut reader(&mut twice), &mut embedder);
        commit(&connection, &again, &vectors, "new", "2")
            .expect("commit")
            .expect("accepted");
        assert_eq!(held(&connection, "alice").len(), 1, "a retry replaces");
    }

    #[test]
    fn what_another_meeting_taught_is_left_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let connection = history();
        connection
            .execute(
                "INSERT INTO meetings (id, started_at, audio_path, created_at, updated_at) \
                 VALUES ('m0', '2023-01-01T00:00:00Z', 'm0.wav', 'now', 'now')",
                [],
            )
            .expect("earlier meeting");
        speaker(&connection, "alice", Some("Alice"));
        segment(&connection, "s1", 1, (1000, 2000), "alice");
        speakers::add_exemplar(
            &connection,
            NewExemplar {
                speaker_id: "alice",
                meeting_id: Some("m0"),
                vector: &[7.0, 7.0],
                model: "new",
                model_version: "2",
                voiced_ms: 500,
                from_operator: true,
                is_negative: false,
                sample: None,
            },
        )
        .expect("other meeting");

        let plan = self::plan(&connection, "m1").expect("plan").expect("audio");
        let mut asked = Vec::new();
        let vectors = embed_ranges(&plan, dir.path(), &mut reader(&mut asked), &mut embedder);
        commit(&connection, &plan, &vectors, "new", "2")
            .expect("commit")
            .expect("accepted");

        let alice = held(&connection, "alice");
        assert_eq!(alice.len(), 2, "one from each Meeting");
        assert!(
            alice
                .iter()
                .any(|exemplar| exemplar.meeting_id.as_deref() == Some("m0")
                    && exemplar.vector == vec![7.0, 7.0]),
            "the earlier Meeting's evidence is untouched"
        );
    }

    /// Embedding is slow and happens outside any lock. Everything that can
    /// move under it is one comparison.
    #[test]
    fn a_record_that_moved_while_embedding_refuses_the_vectors() {
        type Disturbance = fn(&Connection);
        let disturbances: [(&str, Disturbance); 3] = [
            ("a correction landed", |connection: &Connection| {
                speaker(connection, "bob", Some("Bob"));
                correct(connection, "s1", "bob", Some("alice"));
            }),
            ("the Speaker was forgotten", |connection: &Connection| {
                connection
                    .execute("UPDATE speakers SET forgotten = 1 WHERE id = 'alice'", [])
                    .expect("forget");
            }),
            ("the Speaker lost its name", |connection: &Connection| {
                connection
                    .execute(
                        "UPDATE speakers SET display_name = NULL WHERE id = 'alice'",
                        [],
                    )
                    .expect("unname");
            }),
        ];
        for (why, disturb) in disturbances {
            let dir = tempfile::tempdir().expect("tempdir");
            let connection = history();
            speaker(&connection, "alice", Some("Alice"));
            segment(&connection, "s1", 1, (1000, 2000), "alice");

            let plan = self::plan(&connection, "m1").expect("plan").expect("audio");
            let mut asked = Vec::new();
            let vectors = embed_ranges(&plan, dir.path(), &mut reader(&mut asked), &mut embedder);
            disturb(&connection);

            assert_eq!(
                commit(&connection, &plan, &vectors, "new", "2").expect("commit"),
                Err(Refused::Moved),
                "{why}"
            );
            assert!(
                held(&connection, "alice").is_empty(),
                "{why}: and nothing was written"
            );
        }
    }

    #[test]
    fn a_deleted_recording_refuses_rather_than_writing_from_a_stale_plan() {
        let dir = tempfile::tempdir().expect("tempdir");
        let connection = history();
        speaker(&connection, "alice", Some("Alice"));
        segment(&connection, "s1", 1, (1000, 2000), "alice");
        let plan = self::plan(&connection, "m1").expect("plan").expect("audio");
        let mut asked = Vec::new();
        let vectors = embed_ranges(&plan, dir.path(), &mut reader(&mut asked), &mut embedder);

        connection
            .execute("UPDATE meetings SET audio_path = NULL WHERE id = 'm1'", [])
            .expect("kept audio dropped");
        assert_eq!(
            commit(&connection, &plan, &vectors, "new", "2").expect("commit"),
            Err(Refused::Gone)
        );
    }

    /// A Speaker this Meeting no longer says anything about keeps nothing
    /// from it — and the Voiceprint goes with the evidence, or it goes on
    /// recognizing itself from a model nothing else uses.
    ///
    /// The audio is a pseudonym's here, which is the case with no evidence
    /// on either side: a pseudonym is re-minted rather than relearned, so it
    /// gets no ranges, and Alice gets neither a positive nor a negative.
    #[test]
    fn a_former_owner_left_with_nothing_loses_its_voiceprint_too() {
        let dir = tempfile::tempdir().expect("tempdir");
        let connection = history();
        speaker(&connection, "alice", Some("Alice"));
        speaker(&connection, "ghost", None);
        segment(&connection, "s1", 1, (1000, 2000), "ghost");
        speakers::add_exemplar(
            &connection,
            NewExemplar {
                speaker_id: "alice",
                meeting_id: Some("m1"),
                vector: &[1.0, 0.0],
                model: "old",
                model_version: "1",
                voiced_ms: 1000,
                from_operator: false,
                is_negative: false,
                sample: None,
            },
        )
        .expect("what the old model thought");
        speakers::set_voiceprint(&connection, "alice", &[1.0, 0.0], "old", "1").expect("print");

        let plan = self::plan(&connection, "m1").expect("plan").expect("audio");
        assert!(
            plan.ranges.is_empty(),
            "a pseudonym's audio teaches no named voice"
        );
        assert!(
            plan.owners.contains("alice"),
            "Alice is still an owner, because her evidence is what goes"
        );
        let mut asked = Vec::new();
        let vectors = embed_ranges(&plan, dir.path(), &mut reader(&mut asked), &mut embedder);
        commit(&connection, &plan, &vectors, "new", "2")
            .expect("commit")
            .expect("accepted");

        assert!(held(&connection, "alice").is_empty());
        let alice = speakers::get(&connection, "alice")
            .expect("get")
            .expect("row");
        assert!(!alice.has_voiceprint, "the vector went with the evidence");
        assert!(
            held(&connection, "ghost").is_empty(),
            "and the pseudonym was never this writer's to teach"
        );
    }

    #[test]
    fn a_failure_part_way_through_leaves_the_previous_evidence_whole() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut connection = history();
        speaker(&connection, "alice", Some("Alice"));
        segment(&connection, "s1", 1, (1000, 2000), "alice");
        speakers::add_exemplar(
            &connection,
            NewExemplar {
                speaker_id: "alice",
                meeting_id: Some("m1"),
                vector: &[1.0, 0.0],
                model: "old",
                model_version: "1",
                voiced_ms: 1000,
                from_operator: true,
                is_negative: false,
                sample: None,
            },
        )
        .expect("the old set");

        let plan = self::plan(&connection, "m1").expect("plan").expect("audio");
        let mut asked = Vec::new();
        let vectors = embed_ranges(&plan, dir.path(), &mut reader(&mut asked), &mut embedder);

        let transaction = connection.transaction().expect("begin");
        commit(&transaction, &plan, &vectors, "new", "2")
            .expect("commit")
            .expect("accepted");
        drop(transaction);

        let alice = held(&connection, "alice");
        assert_eq!(
            alice.len(),
            1,
            "the old complete set, not a half-written one"
        );
        assert_eq!(alice[0].model, "old");
    }

    /// The end the ticket promises, as far as this can be taken without a
    /// model: clear every Voiceprint, rebuild one Meeting from the ranges,
    /// and the named Speaker is matchable again in the new space.
    ///
    /// The embeddings are synthetic, so what this establishes is the wiring
    /// — the ranges, the replacement, the recomputation and the space the
    /// vectors are stamped with. **Whether a real model's vectors actually
    /// recognize the same person again is measurement, not a test**, and
    /// waits on the model decision this ticket waits on.
    #[test]
    fn a_wipe_then_a_bounded_rebuild_leaves_the_named_voice_matchable_again() {
        let dir = tempfile::tempdir().expect("tempdir");
        let connection = history();
        speaker(&connection, "alice", Some("Alice"));
        segment(&connection, "s1", 1, (1000, 2000), "alice");
        speakers::add_exemplar(
            &connection,
            NewExemplar {
                speaker_id: "alice",
                meeting_id: Some("m1"),
                vector: &[1.0, 0.0],
                model: "old",
                model_version: "1",
                voiced_ms: 1000,
                from_operator: false,
                is_negative: false,
                sample: None,
            },
        )
        .expect("the old space");
        speakers::set_voiceprint(&connection, "alice", &[1.0, 0.0], "old", "1").expect("print");

        // Ticket 05's wipe, as the pending migration performs it.
        connection
            .execute_batch(crate::store::schema::PENDING_MODEL_CHANGE_WIPE)
            .expect("wipe");
        assert!(
            !speakers::get(&connection, "alice")
                .expect("get")
                .expect("row")
                .has_voiceprint,
            "nothing is recognized straight after a model change"
        );

        let plan = self::plan(&connection, "m1").expect("plan").expect("audio");
        let mut asked = Vec::new();
        let vectors = embed_ranges(&plan, dir.path(), &mut reader(&mut asked), &mut embedder);
        commit(&connection, &plan, &vectors, "new", "2")
            .expect("commit")
            .expect("accepted");

        let alice = speakers::get(&connection, "alice")
            .expect("get")
            .expect("row");
        assert!(alice.has_voiceprint, "a Voiceprint again");
        assert_eq!(
            (
                alice.voiceprint_model.as_deref(),
                alice.voiceprint_model_version.as_deref()
            ),
            (Some("new"), Some("2")),
            "and it is in the new space, not the old one"
        );
        let matchable = speakers::voiceprints(&connection, "new", "2").expect("voiceprints");
        let (_, vector, _) = matchable
            .iter()
            .find(|(id, _, _)| id == "alice")
            .expect("matching finds it");
        let mut expected = vectors[0].clone().expect("the range embedded");
        super::super::cluster::l2_normalize(&mut expected);
        assert_eq!(
            *vector, expected,
            "one range, so the Voiceprint is that range's direction"
        );
    }
}
