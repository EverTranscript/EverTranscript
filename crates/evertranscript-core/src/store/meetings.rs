//! Queries over the record.
//!
//! Every function here takes a connection so the caller decides whether it
//! runs on the writer or a reader — the single-writer rule stays visible at
//! the call site rather than hiding behind a repository object.

use anyhow::Result;
use chrono::DateTime;
use chrono::Local;
use evertranscript_protocol::AudioChannel;
use evertranscript_protocol::Meeting;
use evertranscript_protocol::SearchResult;
use evertranscript_protocol::TranscriptSegment;
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use rusqlite::Row;
use rusqlite::params;
use uuid::Uuid;

use super::now_rfc3339;

const MEETING_COLUMNS: &str = "id, started_at, ended_at, title, detected_app, \
                               mirror_filename, audio_path, audio_notes, \
                               calendar_event_id, calendar_attendees";

fn row_to_meeting(row: &Row<'_>) -> rusqlite::Result<Meeting> {
    let started_at: String = row.get(1)?;
    let ended_at: Option<String> = row.get(2)?;
    let duration_seconds = duration_between(&started_at, ended_at.as_deref());
    Ok(Meeting {
        id: row.get(0)?,
        started_at,
        ended_at,
        title: row.get(3)?,
        detected_app: row.get(4)?,
        duration_seconds,
        mirror_filename: row.get(5)?,
        audio_path: row.get(6)?,
        // Stored as JSON. A row written before this column existed, or one
        // holding something unparseable, reads as "nothing was lost" rather
        // than failing the whole Meeting.
        audio_notes: row
            .get::<_, Option<String>>(7)?
            .and_then(|notes| serde_json::from_str(&notes).ok())
            .unwrap_or_default(),
        calendar_event_id: row.get(8)?,
        calendar_attendees: row
            .get::<_, Option<String>>(9)?
            .and_then(|names| serde_json::from_str(&names).ok())
            .unwrap_or_default(),
    })
}

/// Wall-clock length of a finished Meeting, or None while it runs.
fn duration_between(started_at: &str, ended_at: Option<&str>) -> Option<u64> {
    let ended_at = ended_at?;
    let start = DateTime::parse_from_rfc3339(started_at).ok()?;
    let end = DateTime::parse_from_rfc3339(ended_at).ok()?;
    let seconds = (end - start).num_seconds();
    (seconds >= 0).then_some(seconds as u64)
}

/// Starts a Meeting. The id is UUIDv7 — random enough that merging two
/// machines' History folders cannot collide, time-ordered so the record
/// indexes with locality.
pub fn start(
    connection: &Connection,
    title: Option<&str>,
    detected_app: Option<&str>,
) -> Result<Meeting> {
    start_armed(connection, title, detected_app, None, &[])
}

/// Same, carrying what the calendar knew (ADR-0036).
pub fn start_armed(
    connection: &Connection,
    title: Option<&str>,
    detected_app: Option<&str>,
    calendar_event_id: Option<&str>,
    attendees: &[String],
) -> Result<Meeting> {
    let id = Uuid::now_v7().to_string();
    let now = now_rfc3339();
    let attendees = (!attendees.is_empty())
        .then(|| serde_json::to_string(attendees))
        .transpose()?;
    connection.execute(
        "INSERT INTO meetings (id, started_at, title, detected_app, created_at, updated_at, \
                               calendar_event_id, calendar_attendees)
         VALUES (?1, ?2, ?3, ?4, ?2, ?2, ?5, ?6)",
        params![id, now, title, detected_app, calendar_event_id, attendees],
    )?;
    get(connection, &id)?.ok_or_else(|| anyhow::anyhow!("the Meeting vanished after insert"))
}

/// The Meeting in progress, if any. There is at most one.
pub fn active(connection: &Connection) -> Result<Option<Meeting>> {
    let sql = format!(
        "SELECT {MEETING_COLUMNS} FROM meetings WHERE ended_at IS NULL \
         ORDER BY started_at DESC LIMIT 1"
    );
    Ok(connection.query_row(&sql, [], row_to_meeting).optional()?)
}

/// Ends the Meeting and persists it. Story 5: this is the moment a crash can
/// no longer lose the record.
pub fn stop(connection: &Connection, id: &str) -> Result<Meeting> {
    let now = now_rfc3339();
    connection.execute(
        "UPDATE meetings SET ended_at = ?2, updated_at = ?2 WHERE id = ?1 AND ended_at IS NULL",
        params![id, now],
    )?;
    get(connection, id)?.ok_or_else(|| anyhow::anyhow!("no Meeting with id {id}"))
}

pub fn get(connection: &Connection, id: &str) -> Result<Option<Meeting>> {
    let sql = format!("SELECT {MEETING_COLUMNS} FROM meetings WHERE id = ?1");
    Ok(connection
        .query_row(&sql, params![id], row_to_meeting)
        .optional()?)
}

pub fn list(connection: &Connection, limit: u32, offset: u32) -> Result<Vec<Meeting>> {
    let sql = format!(
        "SELECT {MEETING_COLUMNS} FROM meetings ORDER BY started_at DESC LIMIT ?1 OFFSET ?2"
    );
    let mut statement = connection.prepare(&sql)?;
    let meetings = statement
        .query_map(params![limit, offset], row_to_meeting)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(meetings)
}

/// Renames a Meeting. The Mirror is regenerated and renamed by the projection
/// worker, which the update trigger wakes.
pub fn retitle(connection: &Connection, id: &str, title: &str) -> Result<Meeting> {
    let updated = connection.execute(
        "UPDATE meetings SET title = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, title, now_rfc3339()],
    )?;
    if updated == 0 {
        anyhow::bail!("no Meeting with id {id}");
    }
    get(connection, id)?.ok_or_else(|| anyhow::anyhow!("no Meeting with id {id}"))
}

/// Records where the Meeting's audio landed.
pub fn set_audio_path(connection: &Connection, id: &str, audio_path: &str) -> Result<()> {
    connection.execute(
        "UPDATE meetings SET audio_path = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, audio_path, now_rfc3339()],
    )?;
    Ok(())
}

/// Records what this recording lost, so the Meeting says so.
pub fn set_audio_notes(connection: &Connection, id: &str, notes: &[String]) -> Result<()> {
    let encoded = serde_json::to_string(notes)?;
    connection.execute(
        "UPDATE meetings SET audio_notes = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, encoded, now_rfc3339()],
    )?;
    Ok(())
}

/// Records the Mirror this Meeting projects to. Deliberately not covered by
/// the dirty trigger: the projection worker writes this, and dirtying the row
/// it just cleaned would never settle.
pub fn set_mirror_filename(connection: &Connection, id: &str, filename: &str) -> Result<()> {
    connection.execute(
        "UPDATE meetings SET mirror_filename = ?2 WHERE id = ?1",
        params![id, filename],
    )?;
    Ok(())
}

/// Deletes a Meeting from the database, returning what the caller must now
/// remove from disk. Rows cascade; files do not.
pub struct DeletedMeeting {
    pub existed: bool,
    pub mirror_filename: Option<String>,
    pub audio_path: Option<String>,
}

pub fn delete(connection: &Connection, id: &str) -> Result<DeletedMeeting> {
    let Some(meeting) = get(connection, id)? else {
        return Ok(DeletedMeeting {
            existed: false,
            mirror_filename: None,
            audio_path: None,
        });
    };
    connection.execute("DELETE FROM meetings WHERE id = ?1", params![id])?;
    connection.execute(
        "DELETE FROM search_index WHERE meeting_id = ?1",
        params![id],
    )?;
    Ok(DeletedMeeting {
        existed: true,
        mirror_filename: meeting.mirror_filename,
        audio_path: meeting.audio_path,
    })
}

/// A Meeting's Transcript, **with the Operator's corrections applied**.
///
/// The hint join lives in the query rather than at the call sites on
/// purpose. ADR-0009 as amended keeps the machine's conclusion on the
/// segment and layers the Operator's above it, which means every reader that
/// selected `speaker_id` directly would show the Operator a correction they
/// made being ignored — in the Client, the CLI, and the Mirror
/// independently. Making the raw column the harder thing to reach is what
/// stops that from happening three times.
pub fn segments(connection: &Connection, meeting_id: &str) -> Result<Vec<TranscriptSegment>> {
    let mut statement = connection.prepare(
        "SELECT segment.id, segment.sequence, segment.channel, segment.start_ms, segment.end_ms,
                segment.text,
                coalesce(
                    (SELECT hint.speaker_id FROM attribution_hints hint
                      WHERE hint.segment_id = segment.id
                      ORDER BY hint.created_at DESC, hint.id DESC LIMIT 1),
                    segment.speaker_id
                ),
                CASE WHEN EXISTS (
                        SELECT 1 FROM attribution_hints hint
                         WHERE hint.segment_id = segment.id
                     )
                     THEN 'operator'
                     ELSE segment.attribution
                END
           FROM transcript_segments segment
          WHERE segment.meeting_id = ?1
          ORDER BY segment.sequence",
    )?;
    let segments = statement
        .query_map(params![meeting_id], |row| {
            let channel: String = row.get(2)?;
            let attribution: Option<String> = row.get(7)?;
            Ok(TranscriptSegment {
                id: row.get(0)?,
                sequence: row.get(1)?,
                channel: AudioChannel::parse(&channel).unwrap_or(AudioChannel::Mic),
                start_ms: row.get(3)?,
                end_ms: row.get(4)?,
                text: row.get(5)?,
                speaker_id: row.get(6)?,
                attribution: attribution.as_deref().and_then(parse_attribution),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(segments)
}

fn parse_attribution(value: &str) -> Option<evertranscript_protocol::Attribution> {
    use evertranscript_protocol::Attribution;
    match value {
        "voiceprint" => Some(Attribution::Voiceprint),
        "clustered" => Some(Attribution::Clustered),
        "channel" => Some(Attribution::Channel),
        "operator" => Some(Attribution::Operator),
        _ => None,
    }
}

/// Appends a Transcript segment. The record is immutable: segments are only
/// ever added (ADR-0009).
pub fn append_segment(
    connection: &Connection,
    meeting_id: &str,
    channel: AudioChannel,
    start_ms: i64,
    end_ms: i64,
    text: &str,
) -> Result<TranscriptSegment> {
    let next_sequence: i64 = connection.query_row(
        "SELECT coalesce(max(sequence) + 1, 0) FROM transcript_segments WHERE meeting_id = ?1",
        params![meeting_id],
        |row| row.get(0),
    )?;
    let id = Uuid::now_v7().to_string();
    connection.execute(
        "INSERT INTO transcript_segments
           (id, meeting_id, sequence, channel, start_ms, end_ms, text)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            id,
            meeting_id,
            next_sequence,
            channel.as_str(),
            start_ms,
            end_ms,
            text
        ],
    )?;
    Ok(TranscriptSegment {
        id,
        sequence: next_sequence,
        channel,
        start_ms,
        end_ms,
        text: text.to_string(),
        speaker_id: None,
        attribution: None,
    })
}

/// Refreshes a Meeting's row in the search index. Called by the projection
/// worker so what you can find is exactly what the Mirror shows.
pub fn reindex(connection: &Connection, meeting_id: &str, title: &str, body: &str) -> Result<()> {
    connection.execute(
        "DELETE FROM search_index WHERE meeting_id = ?1",
        params![meeting_id],
    )?;
    connection.execute(
        "INSERT INTO search_index (meeting_id, title, body) VALUES (?1, ?2, ?3)",
        params![meeting_id, title, body],
    )?;
    Ok(())
}

/// Turns whatever the Operator typed into a query FTS5 will accept.
///
/// Raw input reaches MATCH as a query language, where an unbalanced quote or
/// a bare `AND` is a syntax error rather than a search. Quoting each token
/// makes any input a literal phrase search, which is what someone typing
/// into a search box means.
fn fts_query(input: &str) -> String {
    let tokens: Vec<String> = input
        .split_whitespace()
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect();
    tokens.join(" ")
}

pub fn search(connection: &Connection, query: &str, limit: u32) -> Result<Vec<SearchResult>> {
    let prepared = fts_query(query);
    if prepared.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT {}, snippet(search_index, 2, '', '', '…', 16) AS snippet
         FROM search_index
         JOIN meetings ON meetings.id = search_index.meeting_id
         WHERE search_index MATCH ?1
         ORDER BY rank
         LIMIT ?2",
        MEETING_COLUMNS
            .split(", ")
            .map(|column| format!("meetings.{column}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let mut statement = connection.prepare(&sql)?;
    let results = statement
        .query_map(params![prepared, limit], |row| {
            Ok(SearchResult {
                meeting: row_to_meeting(row)?,
                // By name, not position: the snippet follows the meeting
                // columns, so a positional index here silently reads the
                // wrong column the moment one is added.
                snippet: row.get("snippet")?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(results)
}

/// Every Meeting whose Mirror is out of date, with the generation the
/// rebuild will acknowledge.
pub fn dirty_meetings(connection: &Connection, limit: u32) -> Result<Vec<(String, i64)>> {
    let mut statement = connection.prepare(
        "SELECT meeting_id, generation FROM mirror_dirty
         WHERE generation > acknowledged_generation LIMIT ?1",
    )?;
    let rows = statement
        .query_map(params![limit], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Marks a rebuild done — but only for the generation it actually rebuilt.
/// A write that landed mid-rebuild bumped the generation, so the row stays
/// dirty and gets rebuilt again instead of going quietly stale.
pub fn acknowledge(connection: &Connection, meeting_id: &str, generation: i64) -> Result<()> {
    connection.execute(
        "UPDATE mirror_dirty SET acknowledged_generation = ?2
         WHERE meeting_id = ?1 AND generation = ?2",
        params![meeting_id, generation],
    )?;
    Ok(())
}

/// The Meeting's calendar date, for its Mirror filename and title fallback.
///
/// Formatted in the offset the timestamp was recorded with, not the machine's
/// current one. A Meeting held at 10:02 in Shanghai stays dated that day after
/// the Operator flies to California — otherwise History would silently
/// re-date, and every Mirror filename with it, on travel.
pub fn local_date(started_at: &str) -> String {
    DateTime::parse_from_rfc3339(started_at)
        .map(|timestamp| timestamp.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|_| Local::now().format("%Y-%m-%d").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> Connection {
        let mut connection = Connection::open_in_memory().expect("open");
        crate::store::schema::configure(&connection).expect("configure");
        crate::store::schema::migrate(&mut connection).expect("migrate");
        connection
    }

    #[test]
    fn a_started_meeting_is_the_active_one_until_stopped() {
        let connection = connection();
        assert!(active(&connection).expect("active").is_none());

        let meeting = start(&connection, None, Some("Zoom")).expect("start");
        let running = active(&connection).expect("active").expect("one running");
        assert_eq!(running.id, meeting.id);
        assert!(running.ended_at.is_none());
        assert!(running.duration_seconds.is_none());

        let stopped = stop(&connection, &meeting.id).expect("stop");
        assert!(stopped.ended_at.is_some());
        assert!(active(&connection).expect("active").is_none());
    }

    #[test]
    fn ids_are_uuid_v7_so_merged_histories_cannot_collide() {
        let connection = connection();
        let first = start(&connection, None, None).expect("start");
        stop(&connection, &first.id).expect("stop");
        let second = start(&connection, None, None).expect("start");

        assert_ne!(first.id, second.id);
        let parsed = Uuid::parse_str(&first.id).expect("a valid uuid");
        assert_eq!(parsed.get_version_num(), 7, "ids must be UUIDv7");
        // v7 is time-ordered, which is what gives the index its locality.
        assert!(first.id < second.id, "ids should sort by creation time");
    }

    #[test]
    fn starting_a_meeting_marks_its_mirror_dirty() {
        let connection = connection();
        let meeting = start(&connection, None, Some("Zoom")).expect("start");
        let dirty = dirty_meetings(&connection, 10).expect("dirty");
        assert_eq!(dirty.len(), 1);
        assert_eq!(dirty[0].0, meeting.id);
    }

    #[test]
    fn acknowledging_a_stale_generation_leaves_the_row_dirty() {
        let connection = connection();
        let meeting = start(&connection, None, None).expect("start");
        let (_, generation) = dirty_meetings(&connection, 10).expect("dirty")[0].clone();

        // A write lands while the rebuild is in flight.
        retitle(&connection, &meeting.id, "Renamed mid-rebuild").expect("retitle");
        acknowledge(&connection, &meeting.id, generation).expect("acknowledge");

        let still_dirty = dirty_meetings(&connection, 10).expect("dirty");
        assert_eq!(
            still_dirty.len(),
            1,
            "a write during the rebuild must keep the Mirror dirty"
        );
    }

    #[test]
    fn writing_the_mirror_filename_does_not_re_dirty_the_row() {
        let connection = connection();
        let meeting = start(&connection, None, None).expect("start");
        let (_, generation) = dirty_meetings(&connection, 10).expect("dirty")[0].clone();

        set_mirror_filename(&connection, &meeting.id, "2026-08-27-zoom-abcd1234.md")
            .expect("set filename");
        acknowledge(&connection, &meeting.id, generation).expect("acknowledge");

        assert!(
            dirty_meetings(&connection, 10).expect("dirty").is_empty(),
            "the projection worker's own write must not restart the loop"
        );
    }

    #[test]
    fn search_finds_indexed_meetings_and_survives_hostile_queries() {
        let connection = connection();
        let meeting = start(&connection, Some("Q3 Budget Review"), None).expect("start");
        reindex(
            &connection,
            &meeting.id,
            "Q3 Budget Review",
            "we agreed to defer the hiring plan until October",
        )
        .expect("reindex");

        let hits = search(&connection, "hiring plan", 10).expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].meeting.id, meeting.id);
        assert!(hits[0].snippet.contains("hiring"));

        // Input that is FTS5 query syntax, not a search term.
        for hostile in ["\"unbalanced", "AND OR NOT", "*", "budget AND", ""] {
            let outcome = search(&connection, hostile, 10);
            assert!(
                outcome.is_ok(),
                "query {hostile:?} must not error: {outcome:?}"
            );
        }
    }

    #[test]
    fn segments_append_in_order_and_never_overwrite() {
        let connection = connection();
        let meeting = start(&connection, None, None).expect("start");

        let first = append_segment(
            &connection,
            &meeting.id,
            AudioChannel::Mic,
            0,
            1200,
            "hello",
        )
        .expect("append");
        let second = append_segment(
            &connection,
            &meeting.id,
            AudioChannel::System,
            1300,
            2500,
            "hi there",
        )
        .expect("append");
        assert_eq!(first.sequence, 0);
        assert_eq!(second.sequence, 1);

        let all = segments(&connection, &meeting.id).expect("segments");
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].text, "hello");
        assert_eq!(all[1].channel, AudioChannel::System);
    }

    #[test]
    fn deleting_reports_the_files_the_caller_must_remove() {
        let connection = connection();
        let meeting = start(&connection, None, None).expect("start");
        set_audio_path(&connection, &meeting.id, ".data/audio/x.m4a").expect("audio");
        set_mirror_filename(&connection, &meeting.id, "2026-08-27-zoom-abcd1234.md")
            .expect("mirror");

        let deleted = delete(&connection, &meeting.id).expect("delete");
        assert!(deleted.existed);
        assert_eq!(deleted.audio_path.as_deref(), Some(".data/audio/x.m4a"));
        assert_eq!(
            deleted.mirror_filename.as_deref(),
            Some("2026-08-27-zoom-abcd1234.md")
        );
        assert!(get(&connection, &meeting.id).expect("get").is_none());
    }

    #[test]
    fn deleting_something_that_is_not_there_is_not_an_error() {
        let connection = connection();
        let deleted = delete(&connection, "no-such-meeting").expect("delete");
        assert!(!deleted.existed);
    }
}
