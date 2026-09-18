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
//! recorded as negative evidence against them (ADR-0009 as amended), so the
//! correction survives the model change rather than being re-derived as a
//! positive from the same audio.
//!
//! **Be precise about what a stored negative does.** It is not a repellent.
//! Nothing scores against one: [`super::cluster::centroid`] filters negatives
//! out, and [`super::cluster::seeds`] reads the positive Voiceprint column,
//! not the exemplar rows — so the only two readers of the flag either ignore
//! the row or never see it. What it does is withhold: the range stops
//! contributing to that Speaker's centroid, and where it was the last usable
//! evidence, the Voiceprint is cleared and the Speaker is not a candidate at
//! all. **Clearing the vector is what withdraws recognition.** The negative
//! does not drive the next rebuild either: [`plan`] derives both signs from
//! the latest hint in `attribution_hints`, never from what is already in
//! `speaker_exemplars`. It is retained as evidence, and current matching
//! ignores it.
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
//! * **pseudonyms and forgotten Speakers.** The first are re-minted and
//!   renumbered rather than relearned, and the second is the whole point of
//!   the mark (ADR-0009).
//!
//! **The Operator was a third of those and is not, since 2026-09-17
//! (DECISIONS Q237).** They relearn from their own attributed ranges like any
//! other named Speaker. ADR-0029's three channel rules still decide who the
//! Operator *is*; what changed is that rule 3's Voiceprint is rebuilt here
//! rather than left for nothing to rebuild. The first real re-run on a
//! populated History is what settled it: a rule that needs a Voiceprint no
//! path restores cannot fire after a model change, and the Operator lost
//! their name on the four most recent Meetings (Q234, Q236). The boundary
//! that stays is the one this whole module is built on — the Operator is
//! relearned from the ranges the record attributes to them, never from a
//! cluster vector and never from another model's guess at which cluster was
//! theirs.
//!
//! # The scope a replacement owns
//!
//! **Every exemplar an eligible Speaker has from this Meeting**, whichever
//! writer produced it. Not a narrower scope tagged for this one: the sibling
//! writer [`crate::store::speakers::correct_attribution`] also writes rows
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
use rusqlite::Transaction;
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

    // Named and not forgotten — the same set `claims` works from, and for
    // the same reasons. A pseudonym is absent because it has no name, so
    // nothing here has to exclude it by hand, and the Operator is in for the
    // reasons in the module doc.
    let eligible: BTreeSet<String> = speakers::relearnable(connection)?
        .into_iter()
        .map(|speaker| speaker.id)
        .collect();

    let segments: Vec<(String, String, i64, i64, Option<String>)> = {
        let mut statement = connection.prepare(
            "SELECT id, channel, start_ms, end_ms, speaker_id FROM transcript_segments \
              WHERE meeting_id = ?1 ORDER BY sequence",
        )?;
        let rows = statement.query_map(params![meeting_id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<_>>()?
    };

    let mut latest_hint = connection.prepare(
        "SELECT speaker_id, replaced_speaker_id FROM attribution_hints \
          WHERE segment_id = ?1 ORDER BY created_at DESC, id DESC LIMIT 1",
    )?;

    // Where the far end was talking, read off the rows already in hand rather
    // than asked for again. A mic range overlapping any of it heard two
    // people, so it teaches neither — ticket 15. **One-directional on
    // purpose** (DECISIONS Q258): the speakers leak into the microphone,
    // while the system channel is a tap of the output stream the mic cannot
    // reach, so a system range is not dropped for the Operator talking across
    // it.
    let far_end: Vec<(i64, i64)> = segments
        .iter()
        .filter(|(_, channel, start, end, _)| {
            AudioChannel::parse(channel) == Some(AudioChannel::System) && end > start
        })
        .map(|(_, _, start, end, _)| (*start, *end))
        .collect();

    let mut ranges = Vec::new();
    for (segment_id, channel, start_ms, end_ms, machine) in segments {
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

        // One read, and a failure is a failure. The newest correction if
        // there is one, the machine's conclusion if there is not — the same
        // order `attributed_speaker` applies, asked once here because the
        // sign, the owner and whether the Operator is behind it all come out
        // of the same row.
        let hint: Option<(String, Option<String>)> = latest_hint
            .query_row(params![&segment_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()?;
        // **A correction, not a non-null replacement.** Attributing a
        // segment the machine had no opinion about records a hint with no
        // `replaced_speaker_id`, and it is still the Operator's act — the
        // strongest evidence there is about that voice. Reading the
        // replacement as the test would file it as the machine's.
        let from_operator = hint.is_some();
        let (owner, replaced) = match hint {
            Some((owner, replaced)) => (Some(owner), replaced),
            None => (machine, None),
        };

        // Positives only. A negative never reaches a centroid — `centroid`
        // filters the flag and `seeds` reads the Voiceprint column — so
        // dropping one would buy nothing and lose the other half of a
        // correction, which the module doc says has to survive the model
        // change.
        let overlapped = channel == AudioChannel::Mic
            && far_end
                .iter()
                .any(|&(start, end)| start < sample.end_ms && end > sample.start_ms);
        if let Some(owner) = owner
            .as_ref()
            .filter(|id| eligible.contains(*id))
            .filter(|_| !overlapped)
        {
            ranges.push(Range {
                segment_id: segment_id.clone(),
                speaker_id: owner.clone(),
                sample,
                is_negative: false,
                from_operator,
            });
        }
        // And who it was taken from. `correct_attribution` records the
        // *machine's* column as the replacement, not the previous hint, so
        // correcting away and back names the same Speaker on both sides —
        // and a Speaker the audio ended up with is not also denied it.
        if let Some(denied) = replaced
            .as_ref()
            .filter(|id| eligible.contains(*id))
            .filter(|id| owner.as_ref() != Some(*id))
        {
            ranges.push(Range {
                segment_id,
                speaker_id: denied.clone(),
                sample,
                is_negative: true,
                from_operator: true,
            });
        }
    }
    drop(latest_hint);

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

/// Embeds each planned range, in order.
///
/// `None` for a range the model legitimately cannot embed — too little
/// voiced audio to run on. **Everything else is an error**, and the whole
/// batch fails: a reader that could not open the recording and a model that
/// failed to run both mean the vectors do not describe this Meeting, and
/// [`commit`] deletes the previous evidence before it writes. Folded into
/// `None`, a transient failure would be indistinguishable from "nothing to
/// learn here" and would replace a Speaker's evidence with nothing while
/// reporting success.
///
/// The same reading and resampling [`super::runner::rebuild`] does, and the
/// model is a callback for the same reason.
pub fn embed_ranges(
    plan: &Plan,
    history_dir: &Path,
    read: &mut Read<'_>,
    embed: &mut super::runner::Embed<'_>,
) -> Result<Vec<Option<Vec<f32>>>> {
    let path = history_dir.join(&plan.audio_path);
    let mut vectors = Vec::with_capacity(plan.ranges.len());
    for range in &plan.ranges {
        let (samples, rate) = read(
            &path,
            range.sample.channel,
            range.sample.start_ms.max(0) as u64,
            range.sample.end_ms.max(0) as u64,
        )
        .map_err(|error| {
            error.context(format!(
                "reading {} of {} to re-embed segment {}",
                range.sample.channel.as_str(),
                plan.audio_path,
                range.segment_id
            ))
        })?;
        vectors.push(
            embed(&super::runner::resample_to_model_rate(&samples, rate)).map_err(|error| {
                anyhow::anyhow!("re-embedding segment {}: {error}", range.segment_id)
            })?,
        );
    }
    Ok(vectors)
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
    /// The vectors are not one per planned range. A caller that lost some
    /// along the way would otherwise delete a complete set of evidence and
    /// put back a partial one.
    Incomplete,
}

/// Replaces everything this Meeting says about its named voices, and
/// recomputes the Voiceprints that changed.
///
/// `vectors` is parallel to `plan.ranges`, and has to be exactly as long:
/// the delete is wholesale, so a short list is a silent erasure rather than
/// a partial write, and it is refused before anything is removed.
///
/// **Takes a [`Transaction`] rather than a connection**, because the delete,
/// the inserts and the recomputation are one change: a failure part-way
/// through has to leave the previous evidence whole rather than a Speaker
/// with half of two models' worth. Its eventual place is inside
/// `finish_run`'s writer closure, where the attribution transaction already
/// is. A comment asking the caller to open one is not the same thing, and
/// every call in autocommit is a partial write waiting for an error.
///
/// **Re-reads the plan first.** Embedding happens outside any lock, and the
/// record can move under it — a correction, a Meeting deleted, a Speaker
/// forgotten or renamed. Comparing the plan it re-reads against the one the
/// vectors were computed for catches every one of those at once, and is the
/// reason this cannot overwrite a newer correction or resurrect evidence for
/// a Speaker somebody has since forgotten.
pub fn commit(
    transaction: &Transaction<'_>,
    plan: &Plan,
    vectors: &[Option<Vec<f32>>],
    model: &str,
    model_version: &str,
) -> Result<std::result::Result<usize, Refused>> {
    if vectors.len() != plan.ranges.len() {
        return Ok(Err(Refused::Incomplete));
    }
    let Some(current) = self::plan(transaction, &plan.meeting_id)? else {
        return Ok(Err(Refused::Gone));
    };
    if current != *plan {
        return Ok(Err(Refused::Moved));
    }

    // Every exemplar these Speakers have from this Meeting, whoever wrote
    // it — see the module docs. Confined to this Meeting, so what they were
    // taught anywhere else stands.
    for speaker_id in &plan.owners {
        transaction.execute(
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
            transaction,
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

    // Including the ones that ended up with nothing, and the ones left with
    // only negatives: `seeds` reads the Voiceprint column, not the rows, so
    // a Speaker whose usable evidence went and whose vector stayed would go
    // on recognizing itself from a model that no longer describes it.
    for speaker_id in &plan.owners {
        super::cluster::refresh_voiceprint(transaction, speaker_id)?;
    }
    Ok(Ok(written))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diarize::{Cluster, Embedding, cluster};
    use crate::store::speakers::Exemplar;

    const RATE: u32 = crate::diarize::fbank::SAMPLE_RATE;

    /// A History with the current schema and one Meeting with kept audio.
    ///
    /// No file behind it: the reader is injected. **What that tests is the
    /// range arithmetic and the sign of each range — which stretches of
    /// which channel the model is shown.** It does not test decoding, and it
    /// is not a substitute for it: whether `audio::sample::read` can open a
    /// real recording is its own question, exercised by the paths that call
    /// it for real, and this build's symphonia carries no WAV reader, so a
    /// synthesized fixture could not have answered it here anyway.
    fn history() -> Connection {
        let mut connection = Connection::open_in_memory().expect("open");
        crate::store::schema::configure(&connection).expect("configure");
        crate::store::schema::migrate(&mut connection).expect("migrate");
        connection
            .execute(
                "INSERT INTO meetings (id, started_at, audio_path, created_at, updated_at) \
                 VALUES ('m1', '2024-01-01T00:00:00Z', 'm1.mp3', 'now', 'now')",
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
        segment_on(connection, id, sequence, "mic", span, owner);
    }

    /// The same, on a named channel — for the fixtures where which channel a
    /// range came from is the point.
    fn segment_on(
        connection: &Connection,
        id: &str,
        sequence: i64,
        channel: &str,
        span: (i64, i64),
        owner: &str,
    ) {
        connection
            .execute(
                "INSERT INTO transcript_segments \
                    (id, meeting_id, sequence, channel, start_ms, end_ms, text, speaker_id) \
                 VALUES (?1, 'm1', ?2, ?3, ?4, ?5, 'hello', ?6)",
                params![id, sequence, channel, span.0, span.1, owner],
            )
            .expect("segment");
    }

    /// A reader that records every stretch it is asked for and answers a
    /// second of audio, and an embedder that turns that into a vector naming
    /// the request. Between them they say exactly what the model was shown.
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

    /// Plan, embed and commit in one step, as the worker eventually will.
    fn rebuild(connection: &mut Connection, dir: &Path) -> std::result::Result<usize, Refused> {
        let plan = self::plan(connection, "m1").expect("plan").expect("audio");
        let mut asked = Vec::new();
        let vectors =
            embed_ranges(&plan, dir, &mut reader(&mut asked), &mut embedder).expect("embed");
        let transaction = connection.transaction().expect("begin");
        let written = commit(&transaction, &plan, &vectors, "new", "2").expect("commit");
        transaction.commit().expect("commit the transaction");
        written
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
        speakers::correct_attribution(&connection, "s2", "alice").expect("correction");

        let plan = self::plan(&connection, "m1").expect("plan").expect("audio");
        let mut asked = Vec::new();
        let vectors =
            embed_ranges(&plan, dir.path(), &mut reader(&mut asked), &mut embedder).expect("embed");

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

        let signs: Vec<(&str, bool, bool)> = plan
            .ranges
            .iter()
            .map(|range| {
                (
                    range.speaker_id.as_str(),
                    range.is_negative,
                    range.from_operator,
                )
            })
            .collect();
        assert_eq!(
            signs,
            vec![
                ("alice", false, false),
                ("alice", false, true),
                ("bob", true, true)
            ],
            "the correction teaches Alice the voice and teaches Bob it is not his, \
             and only the corrected range is the Operator's"
        );
    }

    /// Naming a segment nobody had attributed is still the Operator's act.
    #[test]
    fn attributing_a_segment_the_machine_had_no_opinion_on_is_the_operators_evidence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let connection = history();
        speaker(&connection, "alice", Some("Alice"));
        connection
            .execute(
                "INSERT INTO transcript_segments \
                    (id, meeting_id, sequence, channel, start_ms, end_ms, text, speaker_id) \
                 VALUES ('s1', 'm1', 1, 'mic', 1000, 2000, 'hello', NULL)",
                [],
            )
            .expect("unattributed");
        speakers::correct_attribution(&connection, "s1", "alice").expect("correction");

        let plan = self::plan(&connection, "m1").expect("plan").expect("audio");
        let _ = dir;
        assert_eq!(plan.ranges.len(), 1);
        assert!(
            plan.ranges[0].from_operator,
            "a hint with no replacement is still a hint"
        );
        assert!(!plan.ranges[0].is_negative);
    }

    /// The reversal, through the path production actually uses.
    ///
    /// `correct_attribution` records the *machine's* column as what was
    /// replaced, not the previous hint — so with the machine saying Alice
    /// throughout, Alice → Bob → Alice ends with a hint whose replacement is
    /// Alice herself. She is not denied audio she was just given, and Bob is
    /// left with nothing, because nothing in the record says the voice was
    /// ever his.
    #[test]
    fn a_correction_taken_away_and_given_back_replaces_rather_than_accumulates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut connection = history();
        speaker(&connection, "alice", Some("Alice"));
        speaker(&connection, "bob", Some("Bob"));
        segment(&connection, "s1", 1, (1000, 2000), "alice");

        // Alice is taught first, so the correction below has something to
        // work from: `feed_correction` copies the *existing* exemplars of
        // the Speaker a segment is taken from, and against an empty History
        // it writes nothing at all.
        assert_eq!(rebuild(&mut connection, dir.path()), Ok(1));
        assert_eq!(held(&connection, "alice").len(), 1);

        speakers::correct_attribution(&connection, "s1", "bob").expect("away");
        // The sibling writer's own additions, which are the rows this
        // replacement has to own: Alice's vector copied to Bob as a
        // positive, and turned against Alice as a negative.
        let contradicted = held(&connection, "alice");
        assert_eq!(
            contradicted.len(),
            2,
            "Alice now holds evidence both for and against the same audio"
        );
        assert!(contradicted.iter().any(|exemplar| !exemplar.is_negative));
        assert!(contradicted.iter().any(|exemplar| exemplar.is_negative));
        assert_eq!(held(&connection, "bob").len(), 1, "and Bob holds a copy");

        assert_eq!(
            rebuild(&mut connection, dir.path()),
            Ok(2),
            "Bob's, not Alice's"
        );
        let alice = held(&connection, "alice");
        assert_eq!(
            alice.len(),
            1,
            "the contradiction is resolved, not added to"
        );
        assert!(alice[0].is_negative);

        // And back — which writes nothing, because `correct_attribution`
        // reads `transcript_segments.speaker_id` for what was replaced and
        // that column still says Alice, so `feed_correction` sees
        // `from == to` and returns early. The hint is recorded all the same,
        // and the hint is what the rebuild derives from.
        speakers::correct_attribution(&connection, "s1", "alice").expect("back");
        assert_eq!(
            held(&connection, "alice").len(),
            1,
            "the correction itself added nothing"
        );
        assert_eq!(rebuild(&mut connection, dir.path()), Ok(1));

        let alice = held(&connection, "alice");
        assert_eq!(alice.len(), 1, "the stale negative did not survive");
        assert!(
            !alice[0].is_negative,
            "what is left is the positive the latest correction implies"
        );
        assert_eq!(alice[0].model, "new");
        assert!(
            held(&connection, "bob").is_empty(),
            "and nothing says the voice was ever Bob's"
        );

        // A retry stacks nothing.
        assert_eq!(rebuild(&mut connection, dir.path()), Ok(1));
        assert_eq!(held(&connection, "alice").len(), 1, "a retry replaces");
    }

    /// Correcting a Speaker's last positive segment away leaves it with a
    /// negative and no usable vector — and it is not the Operator forgetting
    /// them.
    #[test]
    fn a_speaker_left_with_only_a_negative_loses_the_vector_but_not_its_identity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut connection = history();
        speaker(&connection, "alice", Some("Alice"));
        speaker(&connection, "bob", Some("Bob"));
        segment(&connection, "s1", 1, (1000, 2000), "alice");

        assert_eq!(rebuild(&mut connection, dir.path()), Ok(1));
        assert!(
            speakers::get(&connection, "alice")
                .expect("get")
                .expect("row")
                .has_voiceprint,
            "Alice is recognizable from her one segment"
        );

        speakers::correct_attribution(&connection, "s1", "bob").expect("it was Bob");
        assert_eq!(rebuild(&mut connection, dir.path()), Ok(2));

        let alice = held(&connection, "alice");
        assert_eq!(alice.len(), 1);
        assert!(alice[0].is_negative, "the negative is kept");
        let row = speakers::get(&connection, "alice")
            .expect("get")
            .expect("row");
        assert!(
            !row.has_voiceprint,
            "and the vector nothing supports is gone"
        );
        assert_eq!(
            row.display_name.as_deref(),
            Some("Alice"),
            "she is still Alice"
        );
        assert!(
            speakers::relearnable(&connection)
                .expect("relearnable")
                .iter()
                .any(|speaker| speaker.id == "alice"),
            "and a recomputation must never mark her forgotten: `relearnable` \
             consults that flag, and she is still someone a later Meeting can teach"
        );
    }

    #[test]
    fn what_another_meeting_taught_is_left_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut connection = history();
        connection
            .execute(
                "INSERT INTO meetings (id, started_at, audio_path, created_at, updated_at) \
                 VALUES ('m0', '2023-01-01T00:00:00Z', 'm0.mp3', 'now', 'now')",
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

        assert_eq!(rebuild(&mut connection, dir.path()), Ok(1));
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
                speakers::correct_attribution(connection, "s1", "bob").expect("correction");
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
            let mut connection = history();
            speaker(&connection, "alice", Some("Alice"));
            segment(&connection, "s1", 1, (1000, 2000), "alice");

            let plan = self::plan(&connection, "m1").expect("plan").expect("audio");
            let mut asked = Vec::new();
            let vectors = embed_ranges(&plan, dir.path(), &mut reader(&mut asked), &mut embedder)
                .expect("embed");
            disturb(&connection);

            let transaction = connection.transaction().expect("begin");
            assert_eq!(
                commit(&transaction, &plan, &vectors, "new", "2").expect("commit"),
                Err(Refused::Moved),
                "{why}"
            );
            transaction.commit().expect("commit");
            assert!(
                held(&connection, "alice").is_empty(),
                "{why}: and nothing was written"
            );
        }
    }

    #[test]
    fn a_deleted_recording_refuses_rather_than_writing_from_a_stale_plan() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut connection = history();
        speaker(&connection, "alice", Some("Alice"));
        segment(&connection, "s1", 1, (1000, 2000), "alice");
        let plan = self::plan(&connection, "m1").expect("plan").expect("audio");
        let mut asked = Vec::new();
        let vectors =
            embed_ranges(&plan, dir.path(), &mut reader(&mut asked), &mut embedder).expect("embed");

        connection
            .execute("UPDATE meetings SET audio_path = NULL WHERE id = 'm1'", [])
            .expect("kept audio dropped");
        let transaction = connection.transaction().expect("begin");
        assert_eq!(
            commit(&transaction, &plan, &vectors, "new", "2").expect("commit"),
            Err(Refused::Gone)
        );
    }

    /// The erasure a zip would have performed quietly.
    #[test]
    fn vectors_that_are_not_one_per_range_are_refused_before_anything_is_deleted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut connection = history();
        speaker(&connection, "alice", Some("Alice"));
        segment(&connection, "s1", 1, (1000, 2000), "alice");
        assert_eq!(rebuild(&mut connection, dir.path()), Ok(1));

        let plan = self::plan(&connection, "m1").expect("plan").expect("audio");
        let transaction = connection.transaction().expect("begin");
        assert_eq!(
            commit(&transaction, &plan, &[], "new", "2").expect("commit"),
            Err(Refused::Incomplete),
            "an empty batch for a non-empty plan is a caller that lost them"
        );
        transaction.commit().expect("commit");
        assert_eq!(
            held(&connection, "alice").len(),
            1,
            "and the complete set it would have erased is still there"
        );
    }

    /// A reader or a model that failed says nothing about this Meeting, and
    /// must not be read as "there was nothing to learn".
    #[test]
    fn a_failed_read_or_a_failed_model_is_an_error_and_changes_nothing() {
        for (why, read_fails) in [
            ("the recording could not be read", true),
            ("the model failed", false),
        ] {
            let dir = tempfile::tempdir().expect("tempdir");
            let mut connection = history();
            speaker(&connection, "alice", Some("Alice"));
            segment(&connection, "s1", 1, (1000, 2000), "alice");
            assert_eq!(rebuild(&mut connection, dir.path()), Ok(1));
            let before = held(&connection, "alice");
            assert_eq!(before.len(), 1);

            let plan = self::plan(&connection, "m1").expect("plan").expect("audio");
            let failed = embed_ranges(
                &plan,
                dir.path(),
                &mut |_path: &Path, channel: AudioChannel, start: u64, end: u64| {
                    if read_fails {
                        anyhow::bail!("no such file");
                    }
                    let _ = (channel, start, end);
                    Ok((vec![0.0; RATE as usize], RATE))
                },
                &mut |_samples: &[f32]| {
                    if read_fails {
                        unreachable!("the read failed first");
                    }
                    Err(crate::diarize::DiarizeError::Failed(anyhow::anyhow!(
                        "the session died"
                    )))
                },
            );
            assert!(failed.is_err(), "{why}");
            assert_eq!(
                held(&connection, "alice"),
                before,
                "{why}: and the evidence in the same space is untouched"
            );
            assert!(
                speakers::get(&connection, "alice")
                    .expect("get")
                    .expect("row")
                    .has_voiceprint,
                "{why}: and so is the Voiceprint"
            );
        }
    }

    /// Q237: a re-run Meeting re-seeds the Operator's Voiceprint.
    ///
    /// This is the test that would have caught Q236. The Operator was
    /// excluded from `plan` until 2026-09-17, so a model change wiped their
    /// Voiceprint and nothing put one back — and rule 3 of ADR-0029 as
    /// amended, the Voiceprint match, is one of the three rules that name
    /// the Operator. On the real History that cost them their name on the
    /// four most recent Meetings (Q234). What is read here is still only
    /// what the record attributes to them, which is the boundary that made
    /// the exclusion look right in the first place.
    #[test]
    fn a_re_run_re_seeds_the_operators_voiceprint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut connection = history();
        speaker(&connection, "me", Some("Me"));
        speakers::set_operator(&connection, "me").expect("operator");
        segment(&connection, "s1", 1, (1000, 2000), "me");

        let plan = self::plan(&connection, "m1").expect("plan").expect("audio");
        assert_eq!(
            plan.ranges.len(),
            1,
            "the Operator's own attributed range is evidence about the Operator"
        );
        assert_eq!(plan.ranges[0].speaker_id, "me");
        assert!(!plan.ranges[0].is_negative, "nobody corrected it away");
        assert!(
            plan.owners.contains("me"),
            "and this Meeting's evidence about them is this run's to replace"
        );

        assert_eq!(rebuild(&mut connection, dir.path()), Ok(1));
        assert_eq!(held(&connection, "me").len(), 1, "one range, one exemplar");
        assert!(
            speakers::get(&connection, "me")
                .expect("get")
                .expect("row")
                .has_voiceprint,
            "and the vector rule 3 matches on is back"
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
    fn a_former_owner_left_with_nothing_loses_its_voiceprint_but_not_its_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut connection = history();
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
        assert_eq!(rebuild(&mut connection, dir.path()), Ok(0));

        assert!(held(&connection, "alice").is_empty());
        let alice = speakers::get(&connection, "alice")
            .expect("get")
            .expect("row");
        assert!(!alice.has_voiceprint, "the vector went with the evidence");
        assert_eq!(
            alice.display_name.as_deref(),
            Some("Alice"),
            "but she is still Alice, not somebody the Operator forgot"
        );
        assert!(
            speakers::relearnable(&connection)
                .expect("relearnable")
                .iter()
                .any(|speaker| speaker.id == "alice"),
            "and a later Meeting may still teach her"
        );
        assert!(
            held(&connection, "ghost").is_empty(),
            "and the pseudonym was never this writer's to teach"
        );
    }

    /// A failure after the delete must put the old set back, whole.
    #[test]
    fn a_failure_after_the_delete_leaves_the_previous_evidence_and_vector_whole() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut connection = history();
        speaker(&connection, "alice", Some("Alice"));
        segment(&connection, "s1", 1, (1000, 2000), "alice");
        assert_eq!(rebuild(&mut connection, dir.path()), Ok(1));
        let before = held(&connection, "alice");
        assert_eq!(before.len(), 1);

        // Nothing may be inserted from here on, so the commit fails after
        // its delete — the one ordering a rollback has to survive.
        connection
            .execute_batch(
                "CREATE TRIGGER no_more_exemplars BEFORE INSERT ON speaker_exemplars                    BEGIN SELECT RAISE(ABORT, 'the disk went away'); END;",
            )
            .expect("trigger");

        let plan = self::plan(&connection, "m1").expect("plan").expect("audio");
        let mut asked = Vec::new();
        let vectors =
            embed_ranges(&plan, dir.path(), &mut reader(&mut asked), &mut embedder).expect("embed");
        let transaction = connection.transaction().expect("begin");
        let failure = commit(&transaction, &plan, &vectors, "newer", "3");
        assert!(failure.is_err(), "the insert failure is propagated");
        drop(transaction);

        connection
            .execute_batch("DROP TRIGGER no_more_exemplars;")
            .expect("drop");
        assert_eq!(
            held(&connection, "alice"),
            before,
            "the old complete set, not a half-written one"
        );
        assert!(
            speakers::get(&connection, "alice")
                .expect("get")
                .expect("row")
                .has_voiceprint,
            "and the Voiceprint it supports"
        );
    }

    /// The end the ticket promises, as far as this can be taken without a
    /// model: clear every Voiceprint, rebuild one Meeting from the ranges,
    /// and the named Speaker is matched again in the new space.
    ///
    /// The embeddings are synthetic, so what this establishes is the wiring
    /// — the ranges, the replacement, the recomputation, and that the
    /// resolver reaches the result. **Whether a real model's vectors
    /// actually recognize the same person again is measurement, not a
    /// test**, and waits on the model decision this ticket waits on.
    #[test]
    fn a_wipe_then_a_bounded_rebuild_leaves_the_named_voice_matchable_again() {
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
                from_operator: false,
                is_negative: false,
                sample: None,
            },
        )
        .expect("the old space");
        speakers::set_voiceprint(&connection, "alice", &[1.0, 0.0], "old", "1").expect("print");

        // Ticket 05's wipe, as the migration performs it.
        connection
            .execute_batch(crate::store::schema::MODEL_CHANGE_WIPE)
            .expect("wipe");
        assert!(
            !speakers::get(&connection, "alice")
                .expect("get")
                .expect("row")
                .has_voiceprint,
            "nothing is recognized straight after a model change"
        );

        assert_eq!(rebuild(&mut connection, dir.path()), Ok(1));

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

        // Through the resolver rather than by reading the gallery: what
        // matters is that the same range, offered as this Meeting's one
        // cluster, now comes back named.
        let seeds = cluster::seeds(&connection, "new", "2").expect("seeds");
        let exemplar = &held(&connection, "alice")[0];
        let clusters: std::collections::BTreeMap<Cluster, Embedding> = [(
            Cluster(0),
            Embedding::new(exemplar.vector.clone(), "new", "2", 1000),
        )]
        .into_iter()
        .collect();
        assert_eq!(
            cluster::resolve(&clusters, &seeds).get(&Cluster(0)),
            Some(&cluster::Resolved::Existing("alice".to_string())),
            "the voice the Meeting taught is the voice it now finds"
        );
    }

    /// Ticket 15: a mic range the far end talked over teaches nobody.
    ///
    /// Asserted through the write path rather than on the query, so what is
    /// pinned is which exemplars a rebuild actually leaves behind.
    #[test]
    fn a_mic_range_the_far_end_talked_over_is_not_enrolled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut connection = history();
        speaker(&connection, "alice", Some("Alice"));
        speaker(&connection, "far", None);
        segment(&connection, "s1", 1, (1000, 2000), "alice");
        segment(&connection, "s2", 2, (3000, 4000), "alice");
        // Across the second one only, and only partly: an intersection is an
        // intersection, there is no threshold to clear.
        segment_on(&connection, "f1", 3, "system", (3500, 3800), "far");

        assert_eq!(rebuild(&mut connection, dir.path()), Ok(1));
        let alice = held(&connection, "alice");
        assert_eq!(alice.len(), 1, "one of her two windows survives");
        assert_eq!(
            alice[0]
                .sample
                .map(|sample| (sample.start_ms, sample.end_ms)),
            Some((1000, 2000)),
            "and it is the one she had to herself"
        );
        assert!(
            speakers::get(&connection, "alice")
                .expect("get")
                .expect("row")
                .has_voiceprint,
            "she is still recognizable from it"
        );
    }

    /// And the rule is one-directional, because the channels are.
    ///
    /// The microphone picks the speakers up, so mic audio can carry the far
    /// end. The system channel is a tap of the output stream, which the
    /// Operator's voice has no route into — so a system range is kept
    /// however much the Operator talked across it (DECISIONS Q258).
    #[test]
    fn a_system_range_the_operator_talked_over_is_still_enrolled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut connection = history();
        speaker(&connection, "alice", Some("Alice"));
        speaker(&connection, "bob", Some("Bob"));
        // Bob is the far end and Alice talks straight across him.
        segment_on(&connection, "f1", 1, "system", (1000, 2000), "bob");
        segment(&connection, "s1", 2, (1200, 1800), "alice");

        assert_eq!(rebuild(&mut connection, dir.path()), Ok(1));
        assert_eq!(
            held(&connection, "bob").len(),
            1,
            "Bob's own recording is not contaminated by Alice talking"
        );
        assert!(
            held(&connection, "alice").is_empty(),
            "and Alice's mic window, which heard them both, teaches nothing"
        );
    }

    /// A Speaker the rule leaves nothing loses recognition and keeps identity.
    ///
    /// The same contract as the negative-only case above, reached the other
    /// way: `clear_voiceprint`, not `delete_voiceprint`, so no forgotten mark
    /// is set and a later Meeting can still teach her.
    #[test]
    fn a_speaker_talked_over_throughout_loses_the_vector_but_not_her_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut connection = history();
        speaker(&connection, "alice", Some("Alice"));
        speaker(&connection, "far", None);
        segment(&connection, "s1", 1, (1000, 2000), "alice");

        assert_eq!(rebuild(&mut connection, dir.path()), Ok(1));
        assert!(
            speakers::get(&connection, "alice")
                .expect("get")
                .expect("row")
                .has_voiceprint,
            "clean to begin with"
        );

        // Now the far end turns out to have been talking over her one window.
        segment_on(&connection, "f1", 2, "system", (1000, 2000), "far");
        assert_eq!(rebuild(&mut connection, dir.path()), Ok(0));

        assert!(
            held(&connection, "alice").is_empty(),
            "the contaminated evidence is withdrawn, not re-derived"
        );
        let row = speakers::get(&connection, "alice")
            .expect("get")
            .expect("row");
        assert!(!row.has_voiceprint, "so the vector goes with it");
        assert_eq!(
            row.display_name.as_deref(),
            Some("Alice"),
            "she is still Alice"
        );
        assert!(
            speakers::relearnable(&connection)
                .expect("relearnable")
                .iter()
                .any(|speaker| speaker.id == "alice"),
            "and still someone a later Meeting can teach — the drop costs \
             recognition, never identity"
        );
    }
}
