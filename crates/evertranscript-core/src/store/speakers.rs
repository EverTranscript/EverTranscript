//! Speakers, Voiceprints, exemplars, and the Operator's corrections.
//!
//! Three ADR commitments are executable here rather than aspirational:
//!
//! - **Speaker records are permanent while the record refers to them; only
//!   Voiceprints are deletable** (ADR-0009 as amended). There is no
//!   `delete_speaker`, and its absence is the feature: deleting a Speaker
//!   somebody's words point at would either orphan those segments or
//!   rewrite the record, and the record does not rewrite. The one deletion
//!   here, [`sweep_unreferenced`], takes only rows nothing points at.
//! - **Naming is confirmation** (ADR-0008 as amended). [`rename`] sets
//!   `confirmed`, because the Operator putting a name to a voice is the
//!   strongest signal the system will ever get about it.
//! - **Corrections append, never overwrite** (ADR-0009 as amended). A
//!   re-assignment writes an `attribution_hints` row; the machine's
//!   conclusion stays on the segment underneath, which is what keeps the
//!   record auditable and re-diarization possible.

use anyhow::Result;
use evertranscript_protocol::AudioChannel;
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use rusqlite::params;
use uuid::Uuid;

use super::now_rfc3339;

/// Why a segment is attributed to whom it is.
///
/// ADR-0008 lists visible match attribution among the legibility surfaces it
/// makes mandatory in exchange for storing biometrics at all. An Operator who
/// cannot ask "why do you think that was Alice?" has no way to know whether
/// to correct it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attribution {
    /// Matched an existing Voiceprint.
    Voiceprint,
    /// Clustered within this Meeting but matched nobody in History — a new
    /// Speaker, honestly labelled as new.
    Clustered,
    /// The mic-channel prior did the work (ADR-0029 as amended).
    Channel,
    /// The Operator said so. Outranks everything above it.
    Operator,
}

impl Attribution {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Voiceprint => "voiceprint",
            Self::Clustered => "clustered",
            Self::Channel => "channel",
            Self::Operator => "operator",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "voiceprint" => Some(Self::Voiceprint),
            "clustered" => Some(Self::Clustered),
            "channel" => Some(Self::Channel),
            "operator" => Some(Self::Operator),
            _ => None,
        }
    }
}

/// A Speaker as the Registry shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Speaker {
    pub id: String,
    /// None until the Operator names it. The pseudonym ("Speaker 3") is a
    /// display concern and is not stored, because storing it would make it
    /// look like a name the Operator chose.
    pub display_name: Option<String>,
    pub is_operator: bool,
    pub has_voiceprint: bool,
    pub voiceprint_model: Option<String>,
    pub voiceprint_model_version: Option<String>,
    pub confirmed: bool,
    /// The Operator deleted this Speaker's Voiceprint, and no re-run may
    /// give it one back. Set by that act and by nothing else — after a model
    /// change "named, no vector" is the ordinary state of most of the
    /// Registry, so the deliberate case needs a mark of its own.
    pub forgotten: bool,
    pub created_at: String,
}

const SPEAKER_COLUMNS: &str = "id, display_name, is_operator, voiceprint IS NOT NULL, \
                               voiceprint_model, voiceprint_model_version, confirmed, \
                               forgotten, created_at";

fn row_to_speaker(row: &rusqlite::Row<'_>) -> rusqlite::Result<Speaker> {
    Ok(Speaker {
        id: row.get(0)?,
        display_name: row.get(1)?,
        is_operator: row.get::<_, i64>(2)? != 0,
        has_voiceprint: row.get::<_, i64>(3)? != 0,
        voiceprint_model: row.get(4)?,
        voiceprint_model_version: row.get(5)?,
        confirmed: row.get::<_, i64>(6)? != 0,
        forgotten: row.get::<_, i64>(7)? != 0,
        created_at: row.get(8)?,
    })
}

/// Creates a Speaker with no name and no Voiceprint yet.
pub fn create(connection: &Connection, is_operator: bool) -> Result<Speaker> {
    let id = Uuid::now_v7().to_string();
    connection.execute(
        "INSERT INTO speakers (id, is_operator, created_at) VALUES (?1, ?2, ?3)",
        params![id, i64::from(is_operator), now_rfc3339()],
    )?;
    get(connection, &id)?.ok_or_else(|| anyhow::anyhow!("the Speaker vanished after insert"))
}

/// The stored id for something a person typed.
///
/// Speakers need this more than Meetings do, not less: one Diarization run
/// mints every Speaker it finds inside the same millisecond, so their ids
/// share a long prefix by construction and the short form the Registry
/// prints is the only thing a person has to type back.
pub fn resolve(connection: &Connection, typed: &str) -> Result<Option<String>> {
    let exact: Option<String> = connection
        .query_row(
            "SELECT id FROM speakers WHERE id = ?1",
            params![typed],
            |row| row.get(0),
        )
        .optional()?;
    if exact.is_some() {
        return Ok(exact);
    }

    let wanted = crate::ids::normalise(typed);
    if wanted.is_empty() {
        return Ok(None);
    }
    // **Contains, not starts-with.** The Registry shows a Speaker by its
    // *tail* (`ids::short_tail`), because ids minted by one Diarization run
    // share their leading characters — so the form a person has to type back
    // is a suffix, and a prefix search would never find it. Matching anywhere
    // costs nothing here: an id that hits two rows is refused either way.
    let mut statement = connection.prepare(
        "SELECT id FROM speakers WHERE replace(lower(id), '-', '') LIKE '%' || ?1 || '%' LIMIT 2",
    )?;
    let mut found = statement
        .query_map(params![wanted], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    match found.len() {
        0 => Ok(None),
        1 => Ok(Some(found.remove(0))),
        _ => anyhow::bail!("{typed} matches more than one Speaker — use more of the id"),
    }
}

pub fn get(connection: &Connection, id: &str) -> Result<Option<Speaker>> {
    let sql = format!("SELECT {SPEAKER_COLUMNS} FROM speakers WHERE id = ?1");
    Ok(connection
        .query_row(&sql, params![id], row_to_speaker)
        .optional()?)
}

/// Every Speaker the app holds — the Registry's inventory (story 30).
///
/// Ordered oldest first, which is UUIDv7 order, so the list is stable across
/// calls and an Operator scrolling it does not see rows move.
pub fn list(connection: &Connection) -> Result<Vec<Speaker>> {
    let sql = format!("SELECT {SPEAKER_COLUMNS} FROM speakers ORDER BY id");
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map([], row_to_speaker)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// The Operator's own Speaker, if it has been established yet.
pub fn operator(connection: &Connection) -> Result<Option<Speaker>> {
    // Ordered even though migration 14's partial unique index makes at most
    // one row matchable. An unordered `LIMIT 1` is how this returned a
    // different Operator than the one the diarize path had just flagged, and
    // an index is a promise about the schema rather than about a query.
    let sql = format!(
        "SELECT {SPEAKER_COLUMNS} FROM speakers WHERE is_operator = 1 \
          ORDER BY created_at, id LIMIT 1"
    );
    Ok(connection.query_row(&sql, [], row_to_speaker).optional()?)
}

/// Moves the Operator flag onto one Speaker, taking it off whoever held it.
///
/// Both halves in one statement pair rather than a bare `SET is_operator =
/// 1`: migration 14's unique index makes a second flagged row an error now,
/// so a caller that only set the flag would fail on the constraint — which
/// is the defect made loud, but still a failure. Clearing first is what
/// makes re-flagging an ordinary move.
pub fn set_operator(connection: &Connection, id: &str) -> Result<()> {
    connection.execute(
        "UPDATE speakers SET is_operator = 0 WHERE is_operator = 1 AND id <> ?1",
        params![id],
    )?;
    let changed = connection.execute(
        "UPDATE speakers SET is_operator = 1 WHERE id = ?1",
        params![id],
    )?;
    if changed == 0 {
        anyhow::bail!("no Speaker with id {id}");
    }
    Ok(())
}

/// Names a Speaker — which also confirms its Voiceprint (ADR-0008 as
/// amended).
///
/// The confirmation is the point. Naming is the only moment the system gets
/// ground truth about a voice, and treating it as a label would throw that
/// away; a confirmed Voiceprint outranks an unconfirmed one when matching.
///
/// The rename propagates retroactively by construction (story 29): segments
/// hold a reference, never a name, so every past appearance follows and the
/// `speakers_after_rename` trigger dirties every affected Mirror.
pub fn rename(connection: &Connection, id: &str, display_name: &str) -> Result<Speaker> {
    let changed = connection.execute(
        "UPDATE speakers SET display_name = ?2, confirmed = 1 WHERE id = ?1",
        params![id, display_name],
    )?;
    if changed == 0 {
        anyhow::bail!("no Speaker with id {id}");
    }
    get(connection, id)?.ok_or_else(|| anyhow::anyhow!("the Speaker vanished after rename"))
}

/// The Speaker holding this exact name, if one does.
///
/// Case-insensitively, because "alice" and "Alice" are the same claim about
/// the same person and a History with both is the duplicate this exists to
/// prevent.
pub fn by_name(connection: &Connection, display_name: &str) -> Result<Option<Speaker>> {
    let sql = format!(
        "SELECT {SPEAKER_COLUMNS} FROM speakers \
         WHERE display_name IS NOT NULL AND display_name = ?1 COLLATE NOCASE \
         ORDER BY id LIMIT 1"
    );
    Ok(connection
        .query_row(&sql, params![display_name], row_to_speaker)
        .optional()?)
}

/// Folds one Speaker into another, and removes the emptied row.
///
/// **Why this exists at all** (ADR-0037, ADR-0008 as amended): the glossary
/// promises that naming a Speaker labels every past appearance. That is
/// false the first time a voice returns as a fresh pseudonym, and a model
/// change makes it certain for every voice at once — the vectors are gone,
/// so everyone comes back a stranger. Without a join, naming the stranger
/// "Alice" produces a second Alice and the promise quietly stops holding.
///
/// Everything that refers to `from` is moved rather than rewritten: segments
/// keep their text and only their reference changes, corrections keep both
/// directions, and exemplars move so the surviving Speaker keeps what both
/// were taught. Then the emptied row goes, because a Speaker nothing refers
/// to displays nowhere and ADR-0009's permanence is about the record, not
/// about rows that no longer describe anybody.
///
/// Refuses to join a Speaker that has a name of its own. Merging two people
/// the Operator has separately identified is the one act here that feels
/// irreversible, and it should be asked for explicitly rather than reached
/// by a rename.
pub fn join(connection: &Connection, from: &str, into: &str) -> Result<Speaker> {
    if from == into {
        anyhow::bail!("a Speaker cannot be joined to itself");
    }
    let source =
        get(connection, from)?.ok_or_else(|| anyhow::anyhow!("no Speaker with id {from}"))?;
    let target =
        get(connection, into)?.ok_or_else(|| anyhow::anyhow!("no Speaker with id {into}"))?;
    if source.display_name.is_some() {
        anyhow::bail!(
            "{from} is already named; joining two named Speakers is not something a rename does"
        );
    }
    if source.is_operator && !target.is_operator {
        anyhow::bail!("the Operator's Speaker cannot be folded into another");
    }

    connection.execute(
        "UPDATE transcript_segments SET speaker_id = ?2 WHERE speaker_id = ?1",
        params![from, into],
    )?;
    connection.execute(
        "UPDATE attribution_hints SET speaker_id = ?2 WHERE speaker_id = ?1",
        params![from, into],
    )?;
    connection.execute(
        "UPDATE attribution_hints SET replaced_speaker_id = ?2 WHERE replaced_speaker_id = ?1",
        params![from, into],
    )?;
    connection.execute(
        "UPDATE speaker_exemplars SET speaker_id = ?2 WHERE speaker_id = ?1",
        params![from, into],
    )?;
    connection.execute("DELETE FROM speakers WHERE id = ?1", params![from])?;

    get(connection, into)?.ok_or_else(|| anyhow::anyhow!("the Speaker vanished after join"))
}

/// Deletes a Speaker's Voiceprint and every exemplar behind it (story 31).
///
/// **The only destructive biometric operation there is** (ADR-0009). The
/// Speaker row, its name, and every segment that references it are untouched:
/// the app stops recognizing the voice, and the record of what was said does
/// not change by one byte. Composed with [`rename`], this is the whole of
/// de-identification (story 32) — which is why no separate anonymize
/// mechanism exists.
///
/// `confirmed` is cleared too. It means "the Operator vouched for this
/// Voiceprint", and there is no longer a Voiceprint to vouch for; leaving it
/// set would make a future re-enrolled vector inherit a confirmation nobody
/// gave it.
///
/// It also marks the Speaker **forgotten**, and it is the only thing that
/// does. After a model change clears every vector (ADR-0037), a Speaker the
/// Operator deliberately forgot is indistinguishable by its columns from one
/// the migration cleared — and a re-run that relearns named Speakers from
/// their attributed segments would bring the forgotten voice back without
/// anyone being told. The mark is what [`relearnable`] consults.
pub fn delete_voiceprint(connection: &Connection, id: &str) -> Result<bool> {
    let changed = connection.execute(
        "UPDATE speakers
            SET voiceprint = NULL, voiceprint_model = NULL,
                voiceprint_model_version = NULL, confirmed = 0, forgotten = 1
          WHERE id = ?1",
        params![id],
    )?;
    connection.execute(
        "DELETE FROM speaker_exemplars WHERE speaker_id = ?1",
        params![id],
    )?;
    Ok(changed > 0)
}

/// Blanks the Voiceprint columns and the confirmation that was about them,
/// leaving the exemplars in place. The half of [`delete_voiceprint`] a
/// recomputation uses when the evidence that is left yields no vector.
pub fn clear_voiceprint(connection: &Connection, id: &str) -> Result<usize> {
    Ok(connection.execute(
        "UPDATE speakers
            SET voiceprint = NULL, voiceprint_model = NULL,
                voiceprint_model_version = NULL, confirmed = 0
          WHERE id = ?1",
        params![id],
    )?)
}

/// The Speakers a re-run may give a Voiceprint back to.
///
/// Every Speaker the Operator has vouched for — named, or the Operator's own
/// row — **except** the ones they deliberately forgot. A re-run rebuilds
/// recognition for the first set from their attributed segments; the second
/// set is the whole reason the `forgotten` mark exists, and leaving them out
/// here is what makes "a deleted Voiceprint stays deleted" true of a bulk
/// re-run rather than only of the moment of deletion.
///
/// Deliberately not a guard on the *correction* path: an Operator
/// re-attributing a segment to a forgotten Speaker is a statement about that
/// one Speaker, made on purpose, and is a different act from a re-run
/// sweeping the voice back in unasked.
pub fn relearnable(connection: &Connection) -> Result<Vec<Speaker>> {
    let sql = format!(
        "SELECT {SPEAKER_COLUMNS} FROM speakers \
          WHERE forgotten = 0 AND (display_name IS NOT NULL OR is_operator = 1) \
          ORDER BY id"
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map([], row_to_speaker)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Speakers nobody has vouched for: unnamed, unconfirmed, not the Operator.
///
/// The whole of what a re-run may replace and a sweep may remove. A name
/// and a confirmation are the Operator's acts (naming *is* confirmation,
/// ADR-0008 as amended), and the Operator's own row is theirs by
/// definition; everything the Operator touched is outside this set by
/// construction rather than by a list of exceptions.
const ANONYMOUS: &str = "is_operator = 0 AND display_name IS NULL AND confirmed = 0";

/// Deletes every anonymous Speaker the record does not refer to: no
/// segment attributed, no correction in either direction, and no exemplar.
///
/// The exemplar clause is what migration 10's one-time prune lacked and why
/// this can stand as a rule where that could not: a Speaker heard only in a
/// Meeting since deleted keeps its exemplars (with no `meeting_id`, see
/// [`super::meetings::delete`]), so "Voiceprints outlive the recordings they
/// came from" holds here untouched. What is left to match is a row with no
/// evidence, no words and no name — the Speakers a re-run withdrew and did
/// not re-attribute — and deleting one changes nothing anything displays.
pub fn sweep_unreferenced(connection: &Connection) -> Result<usize> {
    Ok(connection.execute(
        &format!(
            "DELETE FROM speakers
              WHERE {ANONYMOUS}
                AND id NOT IN (SELECT speaker_id FROM transcript_segments
                                WHERE speaker_id IS NOT NULL)
                AND id NOT IN (SELECT speaker_id FROM attribution_hints)
                AND id NOT IN (SELECT replaced_speaker_id FROM attribution_hints
                                WHERE replaced_speaker_id IS NOT NULL)
                AND id NOT IN (SELECT speaker_id FROM speaker_exemplars)"
        ),
        [],
    )?)
}

/// Sets the current best identity vector for a Speaker.
pub fn set_voiceprint(
    connection: &Connection,
    id: &str,
    vector: &[f32],
    model: &str,
    model_version: &str,
) -> Result<()> {
    connection.execute(
        "UPDATE speakers SET voiceprint = ?2, voiceprint_model = ?3, voiceprint_model_version = ?4 \
         WHERE id = ?1",
        params![id, encode(vector), model, model_version],
    )?;
    Ok(())
}

/// One observation of a voice.
#[derive(Debug, Clone, PartialEq)]
pub struct Exemplar {
    pub id: String,
    pub speaker_id: String,
    pub meeting_id: Option<String>,
    pub vector: Vec<f32>,
    pub model: String,
    pub model_version: String,
    pub voiced_ms: i64,
    pub from_operator: bool,
    pub is_negative: bool,
    /// Where in `meeting_id`'s kept audio this voice can be heard alone.
    /// None for exemplars written before samples were kept.
    pub sample: Option<Sample>,
}

/// One stretch of one channel of a Meeting's kept audio: the voice, on its
/// own, for as long as the clip runs. The coordinates a transcript segment
/// has, so the Registry can cut it out of the recording by arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub channel: AudioChannel,
    pub start_ms: i64,
    pub end_ms: i64,
}

/// A Speaker's playable sample: the oldest exemplar that still has a
/// recording behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleSource {
    pub meeting_id: String,
    pub sample: Sample,
}

/// One observation of a voice, on its way into the record.
///
/// A struct rather than nine positional arguments: the model, its version,
/// the vector and the voiced duration always travel together and are
/// meaningless apart, and a call site that transposed two `bool`s would
/// silently record positive evidence as negative.
#[derive(Debug, Clone)]
pub struct NewExemplar<'a> {
    pub speaker_id: &'a str,
    pub meeting_id: Option<&'a str>,
    pub vector: &'a [f32],
    pub model: &'a str,
    pub model_version: &'a str,
    pub voiced_ms: i64,
    /// True when an Operator correction produced this, which makes it the
    /// strongest evidence the system has about a voice.
    pub from_operator: bool,
    /// Evidence *against*: set when a correction took a segment away from
    /// this Speaker. ADR-0009's amended loop runs in both directions, and
    /// keeping only the positive half lets the same wrong match keep
    /// happening.
    pub is_negative: bool,
    /// Where the voice can be heard, when the source knows.
    pub sample: Option<Sample>,
}

/// Records an observation of a voice.
pub fn add_exemplar(connection: &Connection, exemplar: NewExemplar<'_>) -> Result<String> {
    let id = Uuid::now_v7().to_string();
    connection.execute(
        "INSERT INTO speaker_exemplars
            (id, speaker_id, meeting_id, embedding, model, model_version, voiced_ms, source, \
             is_negative, created_at, sample_channel, sample_start_ms, sample_end_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            id,
            exemplar.speaker_id,
            exemplar.meeting_id,
            encode(exemplar.vector),
            exemplar.model,
            exemplar.model_version,
            exemplar.voiced_ms,
            if exemplar.from_operator {
                "operator"
            } else {
                "machine"
            },
            i64::from(exemplar.is_negative),
            now_rfc3339(),
            exemplar.sample.map(|sample| sample.channel.as_str()),
            exemplar.sample.map(|sample| sample.start_ms),
            exemplar.sample.map(|sample| sample.end_ms),
        ],
    )?;
    Ok(id)
}

/// The anonymous Speakers a Meeting's previous Diarization run taught
/// History about — the ones whose evidence from it a re-run withdraws.
pub fn anonymous_speakers_heard_in(
    connection: &Connection,
    meeting_id: &str,
) -> Result<Vec<String>> {
    let mut statement = connection.prepare(&format!(
        "SELECT DISTINCT speaker_id FROM speaker_exemplars
          WHERE meeting_id = ?1 AND source = 'machine'
            AND speaker_id IN (SELECT id FROM speakers WHERE {ANONYMOUS})
          ORDER BY speaker_id"
    ))?;
    let rows = statement.query_map(params![meeting_id], |row| row.get(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Removes what the machine recorded about one Speaker from one Meeting.
///
/// Only the machine's rows: an exemplar a correction produced is the
/// Operator's evidence and is not the machine's to withdraw. The caller
/// recomputes the Voiceprint afterwards — `seeds` reads the column, and a
/// withdrawal that left the column alone would withdraw nothing.
pub fn delete_machine_exemplars(
    connection: &Connection,
    speaker_id: &str,
    meeting_id: &str,
) -> Result<usize> {
    Ok(connection.execute(
        "DELETE FROM speaker_exemplars
          WHERE speaker_id = ?1 AND meeting_id = ?2 AND source = 'machine'",
        params![speaker_id, meeting_id],
    )?)
}

pub fn exemplars(connection: &Connection, speaker_id: &str) -> Result<Vec<Exemplar>> {
    let mut statement = connection.prepare(
        "SELECT id, speaker_id, meeting_id, embedding, model, model_version, voiced_ms, source, \
                is_negative, sample_channel, sample_start_ms, sample_end_ms
           FROM speaker_exemplars WHERE speaker_id = ?1 ORDER BY id",
    )?;
    let rows = statement.query_map(params![speaker_id], |row| {
        let blob: Vec<u8> = row.get(3)?;
        let source: String = row.get(7)?;
        Ok(Exemplar {
            id: row.get(0)?,
            speaker_id: row.get(1)?,
            meeting_id: row.get(2)?,
            vector: decode(&blob),
            model: row.get(4)?,
            model_version: row.get(5)?,
            voiced_ms: row.get(6)?,
            from_operator: source == "operator",
            is_negative: row.get::<_, i64>(8)? != 0,
            sample: sample_from_row(row, 9)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// The three sample columns starting at `first`, as one value or none.
fn sample_from_row(row: &rusqlite::Row<'_>, first: usize) -> rusqlite::Result<Option<Sample>> {
    let channel: Option<String> = row.get(first)?;
    let (Some(channel), Some(start_ms), Some(end_ms)) = (
        channel.as_deref().and_then(AudioChannel::parse),
        row.get::<_, Option<i64>>(first + 1)?,
        row.get::<_, Option<i64>>(first + 2)?,
    ) else {
        return Ok(None);
    };
    Ok(Some(Sample {
        channel,
        start_ms,
        end_ms,
    }))
}

/// An exemplar whose vector is not from the embedding space in use, and
/// where the audio to rebuild it from is, if it is still here.
#[derive(Debug, Clone, PartialEq)]
pub struct StaleExemplar {
    pub id: String,
    pub speaker_id: String,
    /// The Meeting's kept audio, relative to the History folder, and the
    /// window in it. None when the row never had a window, or its Meeting
    /// was deleted: nothing to rebuild from.
    pub source: Option<(String, Sample)>,
}

/// Every exemplar not embedded by this model and front end.
pub fn stale_exemplars(
    connection: &Connection,
    model: &str,
    model_version: &str,
) -> Result<Vec<StaleExemplar>> {
    let mut statement = connection.prepare(
        "SELECT exemplar.id, exemplar.speaker_id, meeting.audio_path,
                exemplar.sample_channel, exemplar.sample_start_ms, exemplar.sample_end_ms
           FROM speaker_exemplars exemplar
           LEFT JOIN meetings meeting ON meeting.id = exemplar.meeting_id
          WHERE exemplar.model != ?1 OR exemplar.model_version != ?2
          ORDER BY exemplar.id",
    )?;
    let rows = statement.query_map(params![model, model_version], |row| {
        let audio_path: Option<String> = row.get(2)?;
        Ok(StaleExemplar {
            id: row.get(0)?,
            speaker_id: row.get(1)?,
            source: audio_path.zip(sample_from_row(row, 3)?),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Replaces an exemplar's vector with one from the space now in use.
pub fn replace_exemplar_vector(
    connection: &Connection,
    id: &str,
    vector: &[f32],
    model: &str,
    model_version: &str,
) -> Result<()> {
    connection.execute(
        "UPDATE speaker_exemplars SET embedding = ?2, model = ?3, model_version = ?4 WHERE id = ?1",
        params![id, encode(vector), model, model_version],
    )?;
    Ok(())
}

pub fn delete_exemplar(connection: &Connection, id: &str) -> Result<()> {
    connection.execute("DELETE FROM speaker_exemplars WHERE id = ?1", params![id])?;
    Ok(())
}

/// Speakers whose Voiceprint column is from another space, whatever
/// evidence they hold.
pub fn speakers_with_stale_voiceprint(
    connection: &Connection,
    model: &str,
    model_version: &str,
) -> Result<Vec<String>> {
    let mut statement = connection.prepare(
        "SELECT id FROM speakers
          WHERE voiceprint IS NOT NULL
            AND (voiceprint_model IS NOT ?1 OR voiceprint_model_version IS NOT ?2)
          ORDER BY id",
    )?;
    let rows = statement.query_map(params![model, model_version], |row| row.get(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Where a Speaker's voice can be played from, if anywhere.
///
/// The *oldest* exemplar with a window — the capture the Registry's "first
/// heard" line already names — provided its Meeting is still here. An
/// exemplar whose Meeting was deleted keeps its vector and loses its
/// `meeting_id` (the recording is gone, the voice is not), so it drops out of
/// this query on its own and the next capture answers instead.
pub fn sample_source(connection: &Connection, speaker_id: &str) -> Result<Option<SampleSource>> {
    let mut statement = connection.prepare(
        "SELECT meeting_id, sample_channel, sample_start_ms, sample_end_ms
           FROM speaker_exemplars
          WHERE speaker_id = ?1 AND is_negative = 0 AND meeting_id IS NOT NULL
            AND sample_channel IS NOT NULL
          ORDER BY id LIMIT 1",
    )?;
    let found = statement
        .query_row(params![speaker_id], |row| {
            Ok((row.get::<_, String>(0)?, sample_from_row(row, 1)?))
        })
        .optional()?;
    Ok(found
        .and_then(|(meeting_id, sample)| sample.map(|sample| SampleSource { meeting_id, sample })))
}

/// Re-assigns a segment to a different Speaker (story 29b).
///
/// Appends a hint. The machine's `speaker_id` on the segment is deliberately
/// left alone: ADR-0009 as amended keeps the machine's conclusion beneath the
/// Operator's so the record stays auditable and a later re-diarization can
/// still be compared against what was corrected.
pub fn correct_attribution(
    connection: &Connection,
    segment_id: &str,
    speaker_id: &str,
) -> Result<String> {
    let replaced: Option<String> = connection
        .query_row(
            "SELECT speaker_id FROM transcript_segments WHERE id = ?1",
            params![segment_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();

    let id = Uuid::now_v7().to_string();
    connection.execute(
        "INSERT INTO attribution_hints (id, segment_id, speaker_id, replaced_speaker_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, segment_id, speaker_id, replaced, now_rfc3339()],
    )?;
    feed_correction(connection, segment_id, speaker_id, replaced.as_deref())?;
    Ok(id)
}

/// Turns a correction into evidence, in both directions.
///
/// ADR-0009 as amended does not stop at "the display follows the Operator":
/// the correction "feeds the correct Speaker's exemplars as positive and the
/// wrong one's as negative evidence". Only keeping the positive half would
/// leave the system making the same wrong match, every meeting, having been
/// told each time.
///
/// The vector comes from the exemplar the machine recorded for the wrong
/// Speaker **in this Meeting** — that is the observation that produced the
/// mistake, so it is exactly the one worth re-filing. Nothing here has to
/// re-open audio or re-run a model, which is what lets a correction be
/// instantaneous from the Operator's side.
///
/// Called from [`correct_attribution`] rather than left to the caller: a
/// correction that silently failed to teach anything would look identical to
/// one that worked.
fn feed_correction(
    connection: &Connection,
    segment_id: &str,
    to_speaker: &str,
    from_speaker: Option<&str>,
) -> Result<()> {
    let Some(from_speaker) = from_speaker else {
        // Nothing to learn from: the machine had no opinion, so the
        // correction is the first attribution rather than a disagreement.
        return Ok(());
    };
    if from_speaker == to_speaker {
        return Ok(());
    }

    let meeting_id: Option<String> = connection
        .query_row(
            "SELECT meeting_id FROM transcript_segments WHERE id = ?1",
            params![segment_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(meeting_id) = meeting_id else {
        return Ok(());
    };

    let mistaken: Vec<Exemplar> = exemplars(connection, from_speaker)?
        .into_iter()
        .filter(|exemplar| {
            exemplar.meeting_id.as_deref() == Some(meeting_id.as_str()) && !exemplar.is_negative
        })
        .collect();

    for exemplar in mistaken {
        // Positive for the Speaker it actually was, and marked as coming
        // from the Operator, which makes it the strongest evidence the
        // system holds about that voice.
        add_exemplar(
            connection,
            NewExemplar {
                speaker_id: to_speaker,
                meeting_id: Some(&meeting_id),
                vector: &exemplar.vector,
                model: &exemplar.model,
                model_version: &exemplar.model_version,
                voiced_ms: exemplar.voiced_ms,
                from_operator: true,
                is_negative: false,
                // The same stretch of audio — it is this Speaker's voice
                // after all, which is what the correction said.
                sample: exemplar.sample,
            },
        )?;
        // And negative against the Speaker it was not.
        add_exemplar(
            connection,
            NewExemplar {
                speaker_id: from_speaker,
                meeting_id: Some(&meeting_id),
                vector: &exemplar.vector,
                model: &exemplar.model,
                model_version: &exemplar.model_version,
                voiced_ms: exemplar.voiced_ms,
                from_operator: true,
                is_negative: true,
                // Never played back as this Speaker — `sample_source` skips
                // negatives — but kept, so that a front-end or model change
                // can rebuild this evidence from the audio like any other.
                sample: exemplar.sample,
            },
        )?;
    }
    Ok(())
}

/// Who a segment is attributed to, with the Operator's corrections applied.
///
/// This is the display join ADR-0009 describes: the newest hint wins, and the
/// machine's attribution shows through only where the Operator has not said
/// otherwise. Every reader — the Client, the CLI, the Mirror — must go
/// through here, because a reader that queried `speaker_id` directly would
/// show the Operator a correction they made being ignored.
pub fn attributed_speaker(connection: &Connection, segment_id: &str) -> Result<Option<String>> {
    let hinted: Option<String> = connection
        .query_row(
            "SELECT speaker_id FROM attribution_hints WHERE segment_id = ?1 \
             ORDER BY created_at DESC, id DESC LIMIT 1",
            params![segment_id],
            |row| row.get(0),
        )
        .optional()?;
    if hinted.is_some() {
        return Ok(hinted);
    }
    Ok(connection
        .query_row(
            "SELECT speaker_id FROM transcript_segments WHERE id = ?1",
            params![segment_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten())
}

/// Writes the machine's attribution onto a segment.
pub fn attribute_segment(
    connection: &Connection,
    segment_id: &str,
    speaker_id: Option<&str>,
    attribution: Attribution,
) -> Result<()> {
    connection.execute(
        "UPDATE transcript_segments SET speaker_id = ?2, attribution = ?3 WHERE id = ?1",
        params![segment_id, speaker_id, attribution.as_str()],
    )?;
    Ok(())
}

/// How many Meetings a Speaker has been heard in, when the voice was first
/// captured, which Meeting captured it, and when it was last heard — the
/// facts the Registry shows beside a name (ticket 08).
///
/// Derived rather than counted into a column, so they cannot drift from the
/// segments they describe.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Appearances {
    pub meetings: i64,
    /// The start of the earliest Meeting this voice was heard in. Meeting
    /// start rather than the exemplar's own `created_at`, because that one
    /// records when Diarization ran, not when anybody spoke.
    pub first_seen_at: Option<String>,
    pub first_meeting_id: Option<String>,
    pub first_meeting_title: Option<String>,
    pub first_meeting_app: Option<String>,
    /// The start of the most recent one. Equal to `first_seen_at` for a voice
    /// heard in a single Meeting.
    pub last_heard_at: Option<String>,
    pub last_meeting_id: Option<String>,
}

/// A Meeting a voice was heard in, carrying just enough for a Client to name
/// it. Deliberately not a whole [`Meeting`](evertranscript_protocol::Meeting):
/// the Registry wants a label and a way in, and a Speaker heard in fifty
/// Meetings would otherwise drag fifty Summaries and fifty sets of Notes
/// across the socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeardMeeting {
    pub started_at: String,
    pub meeting_id: String,
    pub title: Option<String>,
    pub app: Option<String>,
}

const HEARD_COLUMNS: &str = "meeting.started_at, meeting.id, meeting.title, meeting.detected_app";

fn row_to_heard(row: &rusqlite::Row<'_>) -> rusqlite::Result<HeardMeeting> {
    Ok(HeardMeeting {
        started_at: row.get(0)?,
        meeting_id: row.get(1)?,
        title: row.get(2)?,
        app: row.get(3)?,
    })
}

/// Which Meetings count as an appearance: the machine's attribution, plus the
/// Operator's. A segment that was only ever a correction still puts the
/// Speaker in that Meeting, and counting the machine's column alone would
/// under-report exactly the Speakers the Operator cared enough to fix.
const HEARD_IN: &str = "FROM meetings meeting
      JOIN transcript_segments segment ON segment.meeting_id = meeting.id
     WHERE segment.speaker_id = ?1
        OR segment.id IN (
            SELECT hint.segment_id FROM attribution_hints hint WHERE hint.speaker_id = ?1
        )";

/// The earliest (or most recent) Meeting a voice was heard in, as a row.
///
/// A row rather than bare columns beside a `MIN()`/`MAX()`: SQLite only pins
/// those to the extreme row while exactly one min/max aggregate is in the
/// query, and needing both ends would have quietly taken that guarantee away —
/// leaving a date from one Meeting beside an id from another, with no error.
fn edge_meeting(
    connection: &Connection,
    speaker_id: &str,
    newest: bool,
) -> Result<Option<HeardMeeting>> {
    let order = if newest { "DESC" } else { "ASC" };
    Ok(connection
        .query_row(
            &format!(
                "SELECT {HEARD_COLUMNS} {HEARD_IN} ORDER BY meeting.started_at {order} LIMIT 1"
            ),
            params![speaker_id],
            row_to_heard,
        )
        .optional()?)
}

/// Every Meeting a voice was heard in, newest first — the list behind the
/// Registry's count.
///
/// Not derivable on the Client from the Meetings it already holds: it lists
/// only the most recent few hundred, and the Speaker this exists to serve is
/// exactly the one heard once, a year ago.
pub fn meetings_heard_in(connection: &Connection, speaker_id: &str) -> Result<Vec<HeardMeeting>> {
    let mut statement = connection.prepare(&format!(
        "SELECT DISTINCT {HEARD_COLUMNS} {HEARD_IN} ORDER BY meeting.started_at DESC"
    ))?;
    let rows = statement.query_map(params![speaker_id], row_to_heard)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn appearances(connection: &Connection, speaker_id: &str) -> Result<Appearances> {
    let meetings = connection.query_row(
        &format!("SELECT COUNT(DISTINCT meeting.id) {HEARD_IN}"),
        params![speaker_id],
        |row| row.get(0),
    )?;

    let first = edge_meeting(connection, speaker_id, false)?;
    let last = edge_meeting(connection, speaker_id, true)?;

    Ok(Appearances {
        meetings,
        first_seen_at: first.as_ref().map(|edge| edge.started_at.clone()),
        first_meeting_id: first.as_ref().map(|edge| edge.meeting_id.clone()),
        first_meeting_title: first.as_ref().and_then(|edge| edge.title.clone()),
        first_meeting_app: first.and_then(|edge| edge.app),
        last_heard_at: last.as_ref().map(|edge| edge.started_at.clone()),
        last_meeting_id: last.map(|edge| edge.meeting_id),
    })
}

/// Every Speaker with a Voiceprint in one embedding space, as vectors.
///
/// This is what History offers a new Meeting's clusterer as seeds. All of
/// them rather than a shortlist: the whole promise of ADR-0008 is that a
/// voice from any past Meeting is recognized, and pre-filtering by recency
/// would quietly make "seen once, a year ago" unrecognizable — which is
/// exactly the case retroactive naming exists to serve.
///
/// One space, though: a vector from another model, or from the same model
/// behind a different front end, scores against these as a plausible
/// number that means nothing, and the first front-end fix (DECISIONS Q115)
/// is why this takes the model rather than trusting the column to be
/// comparable. A stale Voiceprint is not offered; it is rebuilt from its
/// kept audio by [`crate::diarize::cluster::adopt_rebuilt`] and offered
/// after.
pub fn voiceprints(
    connection: &Connection,
    model: &str,
    model_version: &str,
) -> Result<Vec<(String, Vec<f32>, bool)>> {
    let mut statement = connection.prepare(
        "SELECT id, voiceprint, confirmed FROM speakers
          WHERE voiceprint IS NOT NULL AND voiceprint_model = ?1
            AND voiceprint_model_version = ?2
          ORDER BY id",
    )?;
    let rows = statement.query_map(params![model, model_version], |row| {
        let blob: Vec<u8> = row.get(1)?;
        Ok((
            row.get::<_, String>(0)?,
            decode(&blob),
            row.get::<_, i64>(2)? != 0,
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Calendar attendee names from Meetings this Speaker appeared in.
///
/// **Candidates, never attributions.** ADR-0036 stores attendees so this can
/// offer them; migration 6's own comment says why they are not applied —
/// an invitation is evidence about who was invited, and turning that into
/// who spoke would be inventing attribution. Naming stays an Operator act.
///
/// Names already used by some Speaker are filtered out, because offering the
/// Operator a name they have already assigned elsewhere invites exactly the
/// duplicate-Speaker mistake the Registry exists to make visible.
pub fn name_suggestions(connection: &Connection, speaker_id: &str) -> Result<Vec<String>> {
    let mut statement = connection.prepare(
        "SELECT DISTINCT meeting.calendar_attendees
           FROM meetings meeting
           JOIN transcript_segments segment ON segment.meeting_id = meeting.id
          WHERE meeting.calendar_attendees IS NOT NULL
            AND (segment.speaker_id = ?1
                 OR segment.id IN (
                     SELECT hint.segment_id FROM attribution_hints hint
                      WHERE hint.speaker_id = ?1
                 ))",
    )?;
    let encoded = statement
        .query_map(params![speaker_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut taken_statement =
        connection.prepare("SELECT display_name FROM speakers WHERE display_name IS NOT NULL")?;
    let taken: std::collections::BTreeSet<String> = taken_statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;

    let mut suggestions = Vec::new();
    for blob in encoded {
        let attendees: Vec<String> = serde_json::from_str(&blob).unwrap_or_default();
        for attendee in attendees {
            if !taken.contains(&attendee) && !suggestions.contains(&attendee) {
                suggestions.push(attendee);
            }
        }
    }
    Ok(suggestions)
}

/// Little-endian f32s. The same encoding on both platforms, because ADR-0035
/// makes the History folder portable and a Voiceprint that meant something
/// different on the other machine would defeat the point of storing it there.
fn encode(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn decode(bytes: &[u8]) -> Vec<f32> {
    let (chunks, _) = bytes.as_chunks::<4>();
    chunks.iter().copied().map(f32::from_le_bytes).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::meetings;

    fn db() -> Connection {
        let mut connection = Connection::open_in_memory().expect("open");
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .expect("fk");
        crate::store::schema::migrate(&mut connection).expect("migrate");
        connection
    }

    #[test]
    fn a_speaker_resolves_from_the_form_the_registry_prints() {
        // **The bug this was written for, reproduced.** A Diarization run
        // mints its Speakers in one burst, and two from the Operator's real
        // registry share twenty-one leading hex characters — so the Registry
        // shows a Speaker by its *tail*, and a prefix search would never find
        // what a person typed back.
        let connection = db();
        let first = create(&connection, false).expect("first");
        let second = create(&connection, false).expect("second");
        assert_ne!(first.id, second.id);

        for speaker in [&first, &second] {
            let tail = crate::ids::short_tail(&speaker.id);
            assert_eq!(
                resolve(&connection, &tail).expect("resolve"),
                Some(speaker.id.clone()),
                "the id the Registry prints ({tail}) must resolve"
            );
            assert_eq!(
                resolve(&connection, &speaker.id).expect("resolve"),
                Some(speaker.id.clone()),
                "and so must the full one"
            );
        }
    }

    #[test]
    fn a_speaker_id_matching_two_is_refused() {
        // **Two real ids from the Operator's Voice Registry**, both produced
        // by one Diarization run. Inserted verbatim rather than generated:
        // an earlier version of this test minted them and asserted they
        // shared twelve characters, which is exactly the millisecond
        // boundary — it passed alone and failed in a full run. A fixture
        // that is sometimes the thing you are testing is not a fixture.
        let connection = db();
        let ids = [
            "01a071fe-55e6-76e0-9571-acea4492076e",
            "01a071fe-55e6-76e0-9571-ad09cb20699f",
        ];
        for id in ids {
            connection
                .execute(
                    "INSERT INTO speakers (id, is_operator, created_at) VALUES (?1, 0, ?2)",
                    params![id, now_rfc3339()],
                )
                .expect("insert");
        }

        // They agree for twenty-one characters, so anything that short is
        // ambiguous and must be refused — this id reaches `delete_voiceprint`.
        assert!(
            resolve(&connection, "01a071fe55e676e09571a").is_err(),
            "a prefix matching two Speakers must be refused, not guessed"
        );

        // And each tail, which is what the Registry actually prints, is not.
        for id in ids {
            assert_eq!(
                resolve(&connection, &crate::ids::short_tail(id)).expect("resolve"),
                Some(id.to_string())
            );
        }
    }

    fn segment(connection: &Connection, meeting_id: &str, sequence: i64) -> String {
        let id = Uuid::now_v7().to_string();
        connection
            .execute(
                "INSERT INTO transcript_segments (id, meeting_id, sequence, channel, start_ms, \
                 end_ms, text) VALUES (?1, ?2, ?3, 'system', 0, 1000, 'hello')",
                params![id, meeting_id, sequence],
            )
            .expect("insert segment");
        id
    }

    #[test]
    fn voiceprints_are_offered_only_in_their_own_space() {
        // A vector from another model, or the same model behind another
        // front end, scores against these as a plausible number that means
        // nothing. The first front-end fix is why this is a query
        // parameter rather than a column to trust.
        let connection = db();
        let old = create(&connection, false).expect("old");
        let new = create(&connection, false).expect("new");
        set_voiceprint(&connection, &old.id, &[1.0, 0.0], "m", "1").expect("old print");
        set_voiceprint(&connection, &new.id, &[0.0, 1.0], "m", "2").expect("new print");

        let offered: Vec<String> = voiceprints(&connection, "m", "2")
            .expect("query")
            .into_iter()
            .map(|(id, _, _)| id)
            .collect();
        assert_eq!(offered, vec![new.id.clone()]);
        assert_eq!(
            speakers_with_stale_voiceprint(&connection, "m", "2").expect("stale"),
            vec![old.id.clone()]
        );
        assert!(
            voiceprints(&connection, "other", "2")
                .expect("query")
                .is_empty()
        );
    }

    #[test]
    fn a_correction_keeps_the_sample_on_both_sides_of_the_evidence() {
        // The negative copy is never played back, but it is evidence like
        // any other, and evidence with no audio behind it cannot follow a
        // model or front-end change.
        use evertranscript_protocol::AudioChannel;
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("meeting");
        meetings::set_audio_path(&connection, &meeting.id, ".data/audio/m.mp3").expect("path");
        let segment_id = segment(&connection, &meeting.id, 1);
        let machine_said = create(&connection, false).expect("john");
        let actually = create(&connection, false).expect("alice");
        let sample = Sample {
            channel: AudioChannel::System,
            start_ms: 5_000,
            end_ms: 9_000,
        };
        add_exemplar(
            &connection,
            NewExemplar {
                speaker_id: &machine_said.id,
                meeting_id: Some(&meeting.id),
                vector: &[1.0, 0.0],
                model: "m",
                model_version: "1",
                voiced_ms: 4_000,
                from_operator: false,
                is_negative: false,
                sample: Some(sample),
            },
        )
        .expect("exemplar");
        attribute_segment(
            &connection,
            &segment_id,
            Some(&machine_said.id),
            Attribution::Voiceprint,
        )
        .expect("attribute");

        correct_attribution(&connection, &segment_id, &actually.id).expect("correct");

        let negative = exemplars(&connection, &machine_said.id)
            .expect("rows")
            .into_iter()
            .find(|exemplar| exemplar.is_negative)
            .expect("a negative");
        assert_eq!(negative.sample, Some(sample));
        assert!(
            sample_source(&connection, &machine_said.id)
                .expect("query")
                .is_some_and(|source| source.sample == sample),
            "the positive original still plays"
        );
        let stale = stale_exemplars(&connection, "m", "2").expect("stale");
        assert_eq!(stale.len(), 3, "the original and both copies");
        assert!(
            stale.iter().all(|exemplar| exemplar.source.is_some()),
            "every one can be rebuilt"
        );
    }

    #[test]
    fn naming_a_speaker_confirms_its_voiceprint() {
        // ADR-0008 as amended: naming is a learning signal, not a label. If
        // this ever becomes a plain UPDATE of display_name, conservative
        // matching loses the only ground truth it ever gets.
        let connection = db();
        let speaker = create(&connection, false).expect("create");
        assert!(!speaker.confirmed);

        let named = rename(&connection, &speaker.id, "Alice").expect("rename");
        assert_eq!(named.display_name.as_deref(), Some("Alice"));
        assert!(named.confirmed, "naming confirms");
    }

    #[test]
    fn deleting_a_voiceprint_keeps_the_speaker_and_the_record() {
        // Story 31, and the boundary ADR-0009 draws: recognition stops, the
        // record does not change. A delete that removed the Speaker would
        // either orphan segments or rewrite history.
        let connection = db();
        let meeting = meetings::start(&connection, Some("Standup"), None).expect("meeting");
        let speaker = create(&connection, false).expect("create");
        rename(&connection, &speaker.id, "Alice").expect("rename");
        set_voiceprint(&connection, &speaker.id, &[0.5, 0.5], "m", "1").expect("voiceprint");
        add_exemplar(
            &connection,
            NewExemplar {
                speaker_id: &speaker.id,
                meeting_id: Some(&meeting.id),
                vector: &[0.5, 0.5],
                model: "m",
                model_version: "1",
                voiced_ms: 4_000,
                from_operator: false,
                is_negative: false,
                sample: None,
            },
        )
        .expect("exemplar");

        let segment_id = segment(&connection, &meeting.id, 1);
        attribute_segment(
            &connection,
            &segment_id,
            Some(&speaker.id),
            Attribution::Voiceprint,
        )
        .expect("attribute");

        assert!(delete_voiceprint(&connection, &speaker.id).expect("delete"));

        let after = get(&connection, &speaker.id).expect("get").expect("exists");
        assert!(!after.has_voiceprint, "recognition stops");
        assert!(!after.confirmed, "nothing left to have vouched for");
        assert_eq!(
            after.display_name.as_deref(),
            Some("Alice"),
            "name survives"
        );
        assert!(exemplars(&connection, &speaker.id).expect("ex").is_empty());

        assert_eq!(
            attributed_speaker(&connection, &segment_id).expect("attr"),
            Some(speaker.id),
            "the record is untouched"
        );
        assert!(
            after.forgotten,
            "and the act left a mark, so a re-run cannot undo it silently"
        );
    }

    #[test]
    fn a_forgotten_speaker_is_never_relearned() {
        // Asserted here rather than through the re-run, so the guarantee
        // does not wait on the re-run being built. Three named Speakers, one
        // of whom the Operator deliberately forgot.
        let connection = db();
        let alice = create(&connection, false).expect("alice");
        rename(&connection, &alice.id, "Alice").expect("name");
        let bob = create(&connection, false).expect("bob");
        rename(&connection, &bob.id, "Bob").expect("name");
        let operator = create(&connection, true).expect("operator");
        let stranger = create(&connection, false).expect("stranger");

        delete_voiceprint(&connection, &bob.id).expect("forget");

        let relearnable: Vec<String> = relearnable(&connection)
            .expect("relearnable")
            .into_iter()
            .map(|speaker| speaker.id)
            .collect();
        assert!(relearnable.contains(&alice.id), "a named voice comes back");
        assert!(
            relearnable.contains(&operator.id),
            "and so does the Operator's own"
        );
        assert!(
            !relearnable.contains(&bob.id),
            "but the forgotten one stays forgotten"
        );
        assert!(
            !relearnable.contains(&stranger.id),
            "and a pseudonym is not something to relearn — it is re-derived"
        );

        assert_eq!(
            get(&connection, &bob.id)
                .expect("get")
                .expect("exists")
                .display_name
                .as_deref(),
            Some("Bob"),
            "forgotten is about the voice, never about the record"
        );
    }

    #[test]
    fn only_forgetting_marks_a_speaker_forgotten() {
        // A Speaker with no Voiceprint is the ordinary state after a model
        // change. The mark has to mean the Operator's act and nothing else,
        // or it means nothing.
        let connection = db();
        let never_enrolled = create(&connection, false).expect("create");
        assert!(!never_enrolled.forgotten);

        rename(&connection, &never_enrolled.id, "Alice").expect("name");
        set_voiceprint(&connection, &never_enrolled.id, &[1.0, 0.0], "m", "1").expect("voiceprint");
        let named = get(&connection, &never_enrolled.id)
            .expect("get")
            .expect("exists");
        assert!(
            !named.forgotten,
            "naming and enrolling leave the mark alone"
        );

        // What a model change does: the vector goes, the mark does not
        // appear.
        connection
            .execute("UPDATE speakers SET voiceprint = NULL", [])
            .expect("clear");
        assert!(
            !get(&connection, &never_enrolled.id)
                .expect("get")
                .expect("exists")
                .forgotten,
            "a cleared Voiceprint is not a forgotten one"
        );
    }

    #[test]
    fn a_correction_wins_the_display_join_without_erasing_the_machine() {
        // ADR-0009 as amended, in one test. The Operator sees their
        // correction; anyone auditing can still see what the machine
        // concluded, which is what makes re-diarization defensible later.
        let connection = db();
        let meeting = meetings::start(&connection, Some("Standup"), None).expect("meeting");
        let machine_said = create(&connection, false).expect("john");
        let operator_says = create(&connection, false).expect("alice");
        let segment_id = segment(&connection, &meeting.id, 1);

        attribute_segment(
            &connection,
            &segment_id,
            Some(&machine_said.id),
            Attribution::Voiceprint,
        )
        .expect("attribute");
        correct_attribution(&connection, &segment_id, &operator_says.id).expect("correct");

        assert_eq!(
            attributed_speaker(&connection, &segment_id).expect("attr"),
            Some(operator_says.id.clone()),
            "the Operator wins the display"
        );

        let beneath: Option<String> = connection
            .query_row(
                "SELECT speaker_id FROM transcript_segments WHERE id = ?1",
                params![segment_id],
                |row| row.get(0),
            )
            .expect("raw");
        assert_eq!(
            beneath,
            Some(machine_said.id),
            "the machine's conclusion is preserved beneath"
        );
    }

    #[test]
    fn the_newest_correction_wins() {
        // An Operator who corrects twice meant the second one.
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("meeting");
        let segment_id = segment(&connection, &meeting.id, 1);
        let first = create(&connection, false).expect("first");
        let second = create(&connection, false).expect("second");

        correct_attribution(&connection, &segment_id, &first.id).expect("first");
        correct_attribution(&connection, &segment_id, &second.id).expect("second");

        assert_eq!(
            attributed_speaker(&connection, &segment_id).expect("attr"),
            Some(second.id)
        );
    }

    #[test]
    fn a_correction_remembers_who_it_replaced() {
        // Needed in both directions: the display shows what changed, and the
        // wrong Speaker gets negative evidence rather than the correction
        // being a one-sided nudge.
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("meeting");
        let segment_id = segment(&connection, &meeting.id, 1);
        let machine_said = create(&connection, false).expect("john");
        let operator_says = create(&connection, false).expect("alice");
        attribute_segment(
            &connection,
            &segment_id,
            Some(&machine_said.id),
            Attribution::Clustered,
        )
        .expect("attribute");
        correct_attribution(&connection, &segment_id, &operator_says.id).expect("correct");

        let replaced: Option<String> = connection
            .query_row(
                "SELECT replaced_speaker_id FROM attribution_hints WHERE segment_id = ?1",
                params![segment_id],
                |row| row.get(0),
            )
            .expect("hint");
        assert_eq!(replaced, Some(machine_said.id));
    }

    #[test]
    fn renaming_a_speaker_dirties_every_meeting_they_appear_in() {
        // Story 29 is only true if the Mirrors follow. The database being
        // right and the folder being stale is the failure nobody notices
        // until they grep History and get the old name.
        let connection = db();
        let first = meetings::start(&connection, Some("One"), None).expect("m1");
        let second = meetings::start(&connection, Some("Two"), None).expect("m2");
        let elsewhere = meetings::start(&connection, Some("Three"), None).expect("m3");
        let speaker = create(&connection, false).expect("speaker");

        for meeting in [&first, &second] {
            let segment_id = segment(&connection, &meeting.id, 1);
            attribute_segment(
                &connection,
                &segment_id,
                Some(&speaker.id),
                Attribution::Clustered,
            )
            .expect("attribute");
        }

        // Everything is dirty from insertion; acknowledge it all first so the
        // rename is the only thing this can be measuring.
        for meeting in [&first, &second, &elsewhere] {
            let generation: i64 = connection
                .query_row(
                    "SELECT generation FROM mirror_dirty WHERE meeting_id = ?1",
                    params![meeting.id],
                    |row| row.get(0),
                )
                .expect("generation");
            meetings::acknowledge(&connection, &meeting.id, generation).expect("ack");
        }

        rename(&connection, &speaker.id, "Alice").expect("rename");

        let dirty: Vec<String> = meetings::dirty_meetings(&connection, 10)
            .expect("dirty")
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert!(dirty.contains(&first.id), "appeared here");
        assert!(dirty.contains(&second.id), "and here");
        assert!(
            !dirty.contains(&elsewhere.id),
            "but not in a Meeting they never spoke in"
        );
    }

    #[test]
    fn a_correction_dirties_the_mirror_too() {
        let connection = db();
        let meeting = meetings::start(&connection, Some("One"), None).expect("meeting");
        let segment_id = segment(&connection, &meeting.id, 1);
        let speaker = create(&connection, false).expect("speaker");

        let generation: i64 = connection
            .query_row(
                "SELECT generation FROM mirror_dirty WHERE meeting_id = ?1",
                params![meeting.id],
                |row| row.get(0),
            )
            .expect("generation");
        meetings::acknowledge(&connection, &meeting.id, generation).expect("ack");

        correct_attribution(&connection, &segment_id, &speaker.id).expect("correct");

        let dirty = meetings::dirty_meetings(&connection, 10).expect("dirty");
        assert!(dirty.iter().any(|(id, _)| id == &meeting.id));
    }

    #[test]
    fn deleting_a_meeting_keeps_its_speakers() {
        // Speakers are cross-Meeting records: they outlive any one of them.
        // A cascade here would mean deleting last Tuesday's standup made the
        // app forget a colleague's voice.
        let connection = db();
        let meeting = meetings::start(&connection, Some("One"), None).expect("meeting");
        let speaker = create(&connection, false).expect("speaker");
        let segment_id = segment(&connection, &meeting.id, 1);
        attribute_segment(
            &connection,
            &segment_id,
            Some(&speaker.id),
            Attribution::Clustered,
        )
        .expect("attribute");
        add_exemplar(
            &connection,
            NewExemplar {
                speaker_id: &speaker.id,
                meeting_id: Some(&meeting.id),
                vector: &[1.0, 0.0],
                model: "m",
                model_version: "1",
                voiced_ms: 3_000,
                from_operator: false,
                is_negative: false,
                sample: None,
            },
        )
        .expect("exemplar");

        meetings::delete(&connection, &meeting.id).expect("delete");

        assert!(get(&connection, &speaker.id).expect("get").is_some());
        let kept = exemplars(&connection, &speaker.id).expect("exemplars");
        assert_eq!(kept.len(), 1, "the voice evidence survives the Meeting");
        assert_eq!(
            kept[0].meeting_id, None,
            "but it no longer claims to come from a Meeting that is gone"
        );

        // And the standing sweep, which runs after every Diarization, must
        // read that surviving exemplar as the reason to leave the row alone.
        assert_eq!(sweep_unreferenced(&connection).expect("sweep"), 0);
        assert!(get(&connection, &speaker.id).expect("get").is_some());
    }

    #[test]
    fn the_sweep_takes_only_what_nothing_refers_to() {
        // The record's every way of pointing at a Speaker, each one enough
        // to keep it: a segment, a correction to it, a correction away from
        // it, an exemplar, a name, being the Operator. Only the row with
        // none of them goes.
        let connection = db();
        let meeting = meetings::start(&connection, Some("One"), None).expect("meeting");

        let attributed = create(&connection, false).expect("attributed");
        let first = segment(&connection, &meeting.id, 1);
        attribute_segment(
            &connection,
            &first,
            Some(&attributed.id),
            Attribution::Clustered,
        )
        .expect("attribute");

        let corrected_to = create(&connection, false).expect("to");
        let corrected_from = create(&connection, false).expect("from");
        let second = segment(&connection, &meeting.id, 2);
        attribute_segment(
            &connection,
            &second,
            Some(&corrected_from.id),
            Attribution::Clustered,
        )
        .expect("attribute");
        correct_attribution(&connection, &second, &corrected_to.id).expect("correct");
        // The machine's conclusion withdrawn afterwards, as a re-run would;
        // the correction still names both of them.
        attribute_segment(&connection, &second, None, Attribution::Clustered).expect("clear");

        let evidenced = create(&connection, false).expect("evidenced");
        add_exemplar(
            &connection,
            NewExemplar {
                speaker_id: &evidenced.id,
                meeting_id: Some(&meeting.id),
                vector: &[1.0, 0.0],
                model: "m",
                model_version: "1",
                voiced_ms: 3_000,
                from_operator: false,
                is_negative: false,
                sample: None,
            },
        )
        .expect("exemplar");

        let named = create(&connection, false).expect("named");
        rename(&connection, &named.id, "Alice").expect("rename");
        let operator = create(&connection, true).expect("operator");
        let nothing = create(&connection, false).expect("nothing");

        assert_eq!(sweep_unreferenced(&connection).expect("sweep"), 1);

        let mut left: Vec<String> = list(&connection)
            .expect("list")
            .into_iter()
            .map(|speaker| speaker.id)
            .collect();
        left.sort();
        let mut expected = vec![
            attributed.id,
            corrected_to.id,
            corrected_from.id,
            evidenced.id,
            named.id,
            operator.id,
        ];
        expected.sort();
        assert_eq!(left, expected, "everything referenced survives");
        assert!(get(&connection, &nothing.id).expect("get").is_none());
    }

    #[test]
    fn a_withdrawal_takes_the_machines_rows_and_leaves_the_operators() {
        let connection = db();
        let meeting = meetings::start(&connection, Some("One"), None).expect("meeting");
        let elsewhere = meetings::start(&connection, Some("Two"), None).expect("meeting");
        let speaker = create(&connection, false).expect("speaker");
        for (meeting_id, from_operator) in [
            (&meeting.id, false),
            (&meeting.id, true),
            (&elsewhere.id, false),
        ] {
            add_exemplar(
                &connection,
                NewExemplar {
                    speaker_id: &speaker.id,
                    meeting_id: Some(meeting_id),
                    vector: &[1.0, 0.0],
                    model: "m",
                    model_version: "1",
                    voiced_ms: 3_000,
                    from_operator,
                    is_negative: false,
                    sample: None,
                },
            )
            .expect("exemplar");
        }
        assert_eq!(
            anonymous_speakers_heard_in(&connection, &meeting.id).expect("heard"),
            vec![speaker.id.clone()]
        );

        assert_eq!(
            delete_machine_exemplars(&connection, &speaker.id, &meeting.id).expect("delete"),
            1
        );

        let kept = exemplars(&connection, &speaker.id).expect("exemplars");
        assert_eq!(kept.len(), 2);
        assert!(
            kept.iter()
                .any(|e| e.meeting_id.as_deref() == Some(meeting.id.as_str()) && e.from_operator),
            "the correction's evidence from this Meeting stays"
        );
        assert!(
            kept.iter()
                .any(|e| e.meeting_id.as_deref() == Some(elsewhere.id.as_str())),
            "and so does what another Meeting taught"
        );

        rename(&connection, &speaker.id, "Alice").expect("rename");
        assert!(
            anonymous_speakers_heard_in(&connection, &meeting.id)
                .expect("heard")
                .is_empty(),
            "a named Speaker is not anonymous, whatever it was heard in"
        );
    }

    #[test]
    fn embeddings_survive_the_round_trip_exactly() {
        // A Voiceprint that changed by one ulp on the way to disk would make
        // matching non-reproducible, and ADR-0035 has these vectors moving
        // between machines.
        let connection = db();
        let speaker = create(&connection, false).expect("create");
        let vector = vec![0.0, -1.0, 0.125, 3.4e-5, f32::MIN_POSITIVE];
        add_exemplar(
            &connection,
            NewExemplar {
                speaker_id: &speaker.id,
                meeting_id: None,
                vector: &vector,
                model: "m",
                model_version: "1",
                voiced_ms: 2_000,
                from_operator: true,
                is_negative: false,
                sample: None,
            },
        )
        .expect("exemplar");
        assert_eq!(
            exemplars(&connection, &speaker.id).expect("ex")[0].vector,
            vector
        );
    }

    #[test]
    fn appearances_count_corrected_segments_too() {
        // If a segment was only ever the Operator's correction, the Speaker
        // was still in that Meeting. Counting only the machine's column
        // would under-report exactly the Speakers the Operator cared enough
        // to fix.
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("meeting");
        let segment_id = segment(&connection, &meeting.id, 1);
        let speaker = create(&connection, false).expect("speaker");
        correct_attribution(&connection, &segment_id, &speaker.id).expect("correct");

        let seen = appearances(&connection, &speaker.id).expect("appearances");
        assert_eq!(seen.meetings, 1);
        assert!(seen.first_seen_at.is_some());
    }

    #[test]
    fn appearances_name_the_meeting_the_voice_was_first_heard_in() {
        // The Registry says *when* a voice was captured and *where*. If the
        // bare columns beside MIN() ever stopped tracking the minimum, the
        // two halves would disagree — a first-heard date from one Meeting
        // and a title from another — and nothing else would catch it.
        let connection = db();
        let earlier = meetings::start(&connection, None, Some("Teams")).expect("m1");
        let later = meetings::start(&connection, Some("Retro"), None).expect("m2");
        connection
            .execute(
                "UPDATE meetings SET started_at = ?2 WHERE id = ?1",
                params![earlier.id, "2026-01-01T09:00:00+00:00"],
            )
            .expect("backdate");
        connection
            .execute(
                "UPDATE meetings SET started_at = ?2 WHERE id = ?1",
                params![later.id, "2026-02-01T09:00:00+00:00"],
            )
            .expect("date");

        let speaker = create(&connection, false).expect("speaker");
        for meeting in [&earlier, &later] {
            let segment_id = segment(&connection, &meeting.id, 1);
            attribute_segment(
                &connection,
                &segment_id,
                Some(&speaker.id),
                Attribution::Clustered,
            )
            .expect("attribute");
        }

        let seen = appearances(&connection, &speaker.id).expect("appearances");
        assert_eq!(seen.meetings, 2);
        assert_eq!(
            seen.first_seen_at.as_deref(),
            Some("2026-01-01T09:00:00+00:00")
        );
        assert_eq!(seen.first_meeting_id.as_deref(), Some(earlier.id.as_str()));
        assert_eq!(seen.first_meeting_app.as_deref(), Some("Teams"));
        assert_eq!(
            seen.first_meeting_title, None,
            "the earlier one is untitled"
        );
        assert_eq!(
            seen.last_heard_at.as_deref(),
            Some("2026-02-01T09:00:00+00:00"),
            "and the last time heard is the later Meeting, not the first"
        );
        assert_eq!(seen.last_meeting_id.as_deref(), Some(later.id.as_str()));

        // The list behind the count: newest first, each Meeting once however
        // many times the voice spoke in it.
        let heard = meetings_heard_in(&connection, &speaker.id).expect("heard in");
        assert_eq!(
            heard
                .iter()
                .map(|meeting| meeting.meeting_id.as_str())
                .collect::<Vec<_>>(),
            vec![later.id.as_str(), earlier.id.as_str()]
        );
    }

    #[test]
    fn a_correction_teaches_both_speakers() {
        // ADR-0009 as amended runs in both directions. Keeping only the
        // positive half would leave the system making the same wrong match
        // every meeting, having been told each time.
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("meeting");
        let segment_id = segment(&connection, &meeting.id, 1);
        let machine_said = create(&connection, false).expect("john");
        let actually = create(&connection, false).expect("alice");

        // The observation that produced the mistake.
        add_exemplar(
            &connection,
            NewExemplar {
                speaker_id: &machine_said.id,
                meeting_id: Some(&meeting.id),
                vector: &[0.6, 0.8],
                model: "m",
                model_version: "1",
                voiced_ms: 5_000,
                from_operator: false,
                is_negative: false,
                sample: None,
            },
        )
        .expect("exemplar");
        attribute_segment(
            &connection,
            &segment_id,
            Some(&machine_said.id),
            Attribution::Voiceprint,
        )
        .expect("attribute");

        correct_attribution(&connection, &segment_id, &actually.id).expect("correct");

        let learned = exemplars(&connection, &actually.id).expect("learned");
        assert_eq!(learned.len(), 1, "the right Speaker gained the evidence");
        assert!(learned[0].from_operator, "and it is Operator-sourced");
        assert!(!learned[0].is_negative);
        assert_eq!(learned[0].vector, vec![0.6, 0.8]);

        let unlearned = exemplars(&connection, &machine_said.id).expect("unlearned");
        assert!(
            unlearned.iter().any(|exemplar| exemplar.is_negative),
            "and the wrong one gained evidence against"
        );
    }

    #[test]
    fn negative_evidence_moves_the_wrong_speakers_voiceprint_away() {
        // The point of recording it. After a correction, recomputing the
        // centroid must no longer include the observation that caused the
        // mistake — otherwise the Voiceprint keeps pointing at the voice it
        // was just told it does not own.
        use crate::diarize::cluster::centroid;
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("meeting");
        let segment_id = segment(&connection, &meeting.id, 1);
        let machine_said = create(&connection, false).expect("john");
        let actually = create(&connection, false).expect("alice");

        for vector in [[1.0_f32, 0.0], [0.0, 1.0]] {
            add_exemplar(
                &connection,
                NewExemplar {
                    speaker_id: &machine_said.id,
                    meeting_id: Some(&meeting.id),
                    vector: &vector,
                    model: "m",
                    model_version: "1",
                    voiced_ms: 4_000,
                    from_operator: false,
                    is_negative: false,
                    sample: None,
                },
            )
            .expect("exemplar");
        }
        attribute_segment(
            &connection,
            &segment_id,
            Some(&machine_said.id),
            Attribution::Voiceprint,
        )
        .expect("attribute");

        correct_attribution(&connection, &segment_id, &actually.id).expect("correct");

        let history: Vec<(Vec<f32>, i64, bool)> = exemplars(&connection, &machine_said.id)
            .expect("exemplars")
            .into_iter()
            .map(|exemplar| (exemplar.vector, exemplar.voiced_ms, exemplar.is_negative))
            .collect();
        assert!(
            history.iter().any(|(_, _, negative)| *negative),
            "negatives are on file"
        );
        assert!(
            centroid(&history).is_some(),
            "and the centroid still computes from what is left"
        );
    }

    #[test]
    fn a_first_attribution_by_the_operator_teaches_nobody_a_lesson() {
        // Correcting a segment the machine never attributed is the Operator
        // filling a gap, not disagreeing. Recording negative evidence
        // against nobody would be inventing a dispute.
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("meeting");
        let segment_id = segment(&connection, &meeting.id, 1);
        let speaker = create(&connection, false).expect("speaker");

        correct_attribution(&connection, &segment_id, &speaker.id).expect("correct");
        assert!(exemplars(&connection, &speaker.id).expect("ex").is_empty());
    }

    #[test]
    fn de_identification_is_rename_plus_voiceprint_delete() {
        // Story 32, composed from parts that already exist. ADR-0009
        // rejected a dedicated anonymize mechanism because rename already is
        // one; this is the test that says the composition actually works.
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("meeting");
        let segment_id = segment(&connection, &meeting.id, 1);
        let speaker = create(&connection, false).expect("speaker");
        rename(&connection, &speaker.id, "Alice Zhang").expect("name");
        set_voiceprint(&connection, &speaker.id, &[1.0, 0.0], "m", "1").expect("voiceprint");
        attribute_segment(
            &connection,
            &segment_id,
            Some(&speaker.id),
            Attribution::Voiceprint,
        )
        .expect("attribute");

        // The Participant asks to be forgotten, to the degree the Operator
        // chooses.
        delete_voiceprint(&connection, &speaker.id).expect("forget the voice");
        rename(&connection, &speaker.id, "Participant 1").expect("forget the name");

        let after = get(&connection, &speaker.id).expect("get").expect("exists");
        assert!(!after.has_voiceprint, "no longer recognized");
        assert_eq!(after.display_name.as_deref(), Some("Participant 1"));
        assert_eq!(
            attributed_speaker(&connection, &segment_id).expect("attr"),
            Some(speaker.id),
            "and what was said is still exactly what was said"
        );
    }

    #[test]
    fn attribution_says_why_and_a_correction_overrides_it() {
        // ADR-0008's visible match attribution. An Operator who cannot ask
        // why has no way to judge whether to correct.
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("meeting");
        let segment_id = segment(&connection, &meeting.id, 1);
        let speaker = create(&connection, false).expect("speaker");
        attribute_segment(
            &connection,
            &segment_id,
            Some(&speaker.id),
            Attribution::Channel,
        )
        .expect("attribute");

        let before = meetings::segments(&connection, &meeting.id).expect("segments");
        assert_eq!(
            before[0].attribution,
            Some(evertranscript_protocol::Attribution::Channel)
        );

        let other = create(&connection, false).expect("other");
        correct_attribution(&connection, &segment_id, &other.id).expect("correct");
        let after = meetings::segments(&connection, &meeting.id).expect("segments");
        assert_eq!(
            after[0].attribution,
            Some(evertranscript_protocol::Attribution::Operator),
            "the Operator's say-so is itself an attribution basis"
        );
    }

    #[test]
    fn joining_moves_every_appearance_onto_the_surviving_speaker() {
        // ADR-0037's reason for existing: after a model change the same
        // voice comes back as a stranger, and naming the stranger must
        // produce one Alice rather than two.
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("meeting");

        let alice = create(&connection, false).expect("alice");
        rename(&connection, &alice.id, "Alice").expect("name");
        let old_segment = segment(&connection, &meeting.id, 1);
        attribute_segment(
            &connection,
            &old_segment,
            Some(alice.id.as_str()),
            crate::store::speakers::Attribution::Clustered,
        )
        .expect("attribute");

        // The same person, back as a pseudonym after the vectors were
        // cleared, heard again and carrying evidence of her own.
        let stranger = create(&connection, false).expect("stranger");
        let new_segment = segment(&connection, &meeting.id, 2);
        attribute_segment(
            &connection,
            &new_segment,
            Some(stranger.id.as_str()),
            crate::store::speakers::Attribution::Clustered,
        )
        .expect("attribute");
        add_exemplar(
            &connection,
            NewExemplar {
                speaker_id: &stranger.id,
                meeting_id: Some(&meeting.id),
                vector: &[1.0, 0.0],
                model: "m",
                model_version: "1",
                voiced_ms: 30_000,
                from_operator: false,
                is_negative: false,
                sample: None,
            },
        )
        .expect("exemplar");

        let survivor = join(&connection, &stranger.id, &alice.id).expect("join");
        assert_eq!(survivor.id, alice.id, "the named Speaker survives");
        assert!(
            get(&connection, &stranger.id).expect("get").is_none(),
            "the pseudonymous row is swept"
        );
        assert_eq!(
            attributed_speaker(&connection, &new_segment)
                .expect("attributed")
                .as_deref(),
            Some(alice.id.as_str()),
            "the segment follows rather than being rewritten"
        );
        assert_eq!(
            exemplars(&connection, &alice.id).expect("exemplars").len(),
            1,
            "what the pseudonym was taught moves too"
        );
    }

    #[test]
    fn joining_one_named_speaker_into_another_is_refused() {
        // The one act here that feels irreversible stays the one explicitly
        // asked for, rather than something a rename can reach.
        let connection = db();
        let alice = create(&connection, false).expect("alice");
        rename(&connection, &alice.id, "Alice").expect("name");
        let bob = create(&connection, false).expect("bob");
        rename(&connection, &bob.id, "Bob").expect("name");

        assert!(join(&connection, &bob.id, &alice.id).is_err());
        assert!(get(&connection, &bob.id).expect("get").is_some());
    }

    #[test]
    fn a_name_already_held_is_found_whatever_its_case() {
        // "alice" and "Alice" are the same claim about the same person, and
        // a History holding both is the duplicate the join exists to stop.
        let connection = db();
        let alice = create(&connection, false).expect("alice");
        rename(&connection, &alice.id, "Alice").expect("name");

        assert_eq!(
            by_name(&connection, "alice")
                .expect("by_name")
                .map(|s| s.id),
            Some(alice.id)
        );
        assert!(by_name(&connection, "Bob").expect("by_name").is_none());
    }
}
