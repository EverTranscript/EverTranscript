//! Speakers, Voiceprints, exemplars, and the Operator's corrections.
//!
//! Three ADR commitments are executable here rather than aspirational:
//!
//! - **Speaker records are permanent; only Voiceprints are deletable**
//!   (ADR-0009). There is no `delete_speaker`, and its absence is the
//!   feature: deleting a Speaker would either orphan every segment that
//!   references it or rewrite the record, and the record does not rewrite.
//! - **Naming is confirmation** (ADR-0008 as amended). [`rename`] sets
//!   `confirmed`, because the Operator putting a name to a voice is the
//!   strongest signal the system will ever get about it.
//! - **Corrections append, never overwrite** (ADR-0009 as amended). A
//!   re-assignment writes an [`attribution_hints`] row; the machine's
//!   conclusion stays on the segment underneath, which is what keeps the
//!   record auditable and re-diarization possible.

use anyhow::Result;
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
    pub created_at: String,
}

const SPEAKER_COLUMNS: &str = "id, display_name, is_operator, voiceprint IS NOT NULL, \
                               voiceprint_model, voiceprint_model_version, confirmed, created_at";

fn row_to_speaker(row: &rusqlite::Row<'_>) -> rusqlite::Result<Speaker> {
    Ok(Speaker {
        id: row.get(0)?,
        display_name: row.get(1)?,
        is_operator: row.get::<_, i64>(2)? != 0,
        has_voiceprint: row.get::<_, i64>(3)? != 0,
        voiceprint_model: row.get(4)?,
        voiceprint_model_version: row.get(5)?,
        confirmed: row.get::<_, i64>(6)? != 0,
        created_at: row.get(7)?,
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
    let sql = format!("SELECT {SPEAKER_COLUMNS} FROM speakers WHERE is_operator = 1 LIMIT 1");
    Ok(connection.query_row(&sql, [], row_to_speaker).optional()?)
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
pub fn delete_voiceprint(connection: &Connection, id: &str) -> Result<bool> {
    let changed = connection.execute(
        "UPDATE speakers
            SET voiceprint = NULL, voiceprint_model = NULL,
                voiceprint_model_version = NULL, confirmed = 0
          WHERE id = ?1",
        params![id],
    )?;
    connection.execute(
        "DELETE FROM speaker_exemplars WHERE speaker_id = ?1",
        params![id],
    )?;
    Ok(changed > 0)
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
}

/// Records an observation of a voice.
pub fn add_exemplar(connection: &Connection, exemplar: NewExemplar<'_>) -> Result<String> {
    let id = Uuid::now_v7().to_string();
    connection.execute(
        "INSERT INTO speaker_exemplars
            (id, speaker_id, meeting_id, embedding, model, model_version, voiced_ms, source, \
             is_negative, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
        ],
    )?;
    Ok(id)
}

pub fn exemplars(connection: &Connection, speaker_id: &str) -> Result<Vec<Exemplar>> {
    let mut statement = connection.prepare(
        "SELECT id, speaker_id, meeting_id, embedding, model, model_version, voiced_ms, source, \
                is_negative
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
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
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

/// How many Meetings a Speaker has been heard in, and when — the facts the
/// Registry shows beside a name (ticket 08).
///
/// Derived rather than counted into a column, so it cannot drift from the
/// segments it describes.
pub fn appearances(connection: &Connection, speaker_id: &str) -> Result<(i64, Option<String>)> {
    let row = connection.query_row(
        "SELECT COUNT(DISTINCT meeting.id), MIN(meeting.started_at)
           FROM meetings meeting
           JOIN transcript_segments segment ON segment.meeting_id = meeting.id
          WHERE segment.speaker_id = ?1
             OR segment.id IN (
                 SELECT hint.segment_id FROM attribution_hints hint WHERE hint.speaker_id = ?1
             )",
        params![speaker_id],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)),
    )?;
    Ok(row)
}

/// Every Speaker with a Voiceprint, as vectors.
///
/// This is what History offers a new Meeting's clusterer as seeds. All of
/// them rather than a shortlist: the whole promise of ADR-0008 is that a
/// voice from any past Meeting is recognized, and pre-filtering by recency
/// would quietly make "seen once, a year ago" unrecognizable — which is
/// exactly the case retroactive naming exists to serve.
pub fn voiceprints(connection: &Connection) -> Result<Vec<(String, Vec<f32>, bool)>> {
    let mut statement = connection.prepare(
        "SELECT id, voiceprint, confirmed FROM speakers WHERE voiceprint IS NOT NULL ORDER BY id",
    )?;
    let rows = statement.query_map([], |row| {
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

        let (count, first_seen) = appearances(&connection, &speaker.id).expect("appearances");
        assert_eq!(count, 1);
        assert!(first_seen.is_some());
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
}
