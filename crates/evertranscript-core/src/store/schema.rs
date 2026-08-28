//! The record's schema and its migrations.
//!
//! House rules, borrowed from anarlog's schema hygiene and kept deliberately:
//! every table is STRICT, every enum is a CHECK, and invariants that matter
//! are constraints rather than conventions — a rule the database enforces
//! cannot be forgotten by a future writer.

use rusqlite::Connection;

/// Ordered migrations. Append only; never edit a shipped one.
const MIGRATIONS: &[&str] = &[
    // 1 — the record: Meetings, their Transcript segments, and the Speakers
    // attribution will point at. Voiceprint columns exist from the start so
    // M3 adds behavior, not a table rewrite.
    r#"
    CREATE TABLE meetings (
        id               TEXT PRIMARY KEY NOT NULL,
        started_at       TEXT NOT NULL,
        ended_at         TEXT,
        title            TEXT,
        detected_app     TEXT,
        mirror_filename  TEXT,
        audio_path       TEXT,
        created_at       TEXT NOT NULL,
        updated_at       TEXT NOT NULL
    ) STRICT;

    CREATE TABLE speakers (
        id                       TEXT PRIMARY KEY NOT NULL,
        display_name             TEXT,
        is_operator              INTEGER NOT NULL DEFAULT 0 CHECK (is_operator IN (0, 1)),
        -- ADR-0035: the Voiceprint lives here, in the record, unencrypted, so
        -- copying the History folder moves recognition with it.
        voiceprint               BLOB,
        voiceprint_model         TEXT,
        voiceprint_model_version TEXT,
        -- Set when the Operator names the Speaker: naming is confirmation,
        -- and confirmed Voiceprints outrank unconfirmed ones when matching
        -- (ADR-0008 as amended).
        confirmed                INTEGER NOT NULL DEFAULT 0 CHECK (confirmed IN (0, 1)),
        created_at               TEXT NOT NULL
    ) STRICT;

    CREATE TABLE transcript_segments (
        id          TEXT PRIMARY KEY NOT NULL,
        meeting_id  TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
        sequence    INTEGER NOT NULL,
        channel     TEXT NOT NULL CHECK (channel IN ('mic', 'system')),
        start_ms    INTEGER NOT NULL,
        end_ms      INTEGER NOT NULL,
        text        TEXT NOT NULL,
        speaker_id  TEXT REFERENCES speakers(id) ON DELETE SET NULL,
        UNIQUE (meeting_id, sequence)
    ) STRICT;

    CREATE INDEX transcript_segments_meeting ON transcript_segments(meeting_id, start_ms);
    CREATE INDEX meetings_started_at ON meetings(started_at DESC);
    "#,
    // 2 — the Mirror projection queue. Triggers mark a Meeting dirty; one
    // worker rebuilds and acks. A write landing mid-rebuild bumps the
    // generation again, so the ack does not clear it and the Mirror is
    // rebuilt once more rather than silently going stale.
    r#"
    CREATE TABLE mirror_dirty (
        meeting_id              TEXT PRIMARY KEY NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
        generation              INTEGER NOT NULL DEFAULT 1,
        acknowledged_generation INTEGER NOT NULL DEFAULT 0
    ) STRICT;

    CREATE TRIGGER meetings_after_insert AFTER INSERT ON meetings BEGIN
        INSERT INTO mirror_dirty (meeting_id, generation) VALUES (NEW.id, 1)
        ON CONFLICT (meeting_id) DO UPDATE SET generation = generation + 1;
    END;

    -- Deliberately scoped with UPDATE OF: the worker itself writes
    -- mirror_filename, and a trigger that fired on that write would dirty the
    -- row it just cleaned and never settle.
    CREATE TRIGGER meetings_after_update
    AFTER UPDATE OF title, started_at, ended_at, detected_app, audio_path ON meetings BEGIN
        INSERT INTO mirror_dirty (meeting_id, generation) VALUES (NEW.id, 1)
        ON CONFLICT (meeting_id) DO UPDATE SET generation = generation + 1;
    END;

    CREATE TRIGGER segments_after_insert AFTER INSERT ON transcript_segments BEGIN
        INSERT INTO mirror_dirty (meeting_id, generation) VALUES (NEW.meeting_id, 1)
        ON CONFLICT (meeting_id) DO UPDATE SET generation = generation + 1;
    END;

    CREATE TRIGGER segments_after_update AFTER UPDATE ON transcript_segments BEGIN
        INSERT INTO mirror_dirty (meeting_id, generation) VALUES (NEW.meeting_id, 1)
        ON CONFLICT (meeting_id) DO UPDATE SET generation = generation + 1;
    END;
    "#,
    // 3 — full-text search over the same projection the Mirror renders, so
    // what you can find is exactly what you can read.
    r#"
    CREATE VIRTUAL TABLE search_index USING fts5(
        meeting_id UNINDEXED,
        title,
        body
    );
    "#,
    // 4 — what a recording lost, in the record rather than only in a log.
    // A Meeting captured with half its audio previously looked exactly like
    // a complete one; the Operator opened one-sided notes with nothing to
    // explain them. A JSON array of human-readable notes, empty when the
    // recording was whole.
    r#"
    ALTER TABLE meetings ADD COLUMN audio_notes TEXT;
    "#,
    // 5 — the Watchlist: what Meeting Detection watches on this machine
    // (ADR-0024, ADR-0030). In the machine store rather than the History
    // folder, like settings: the list describes this installation, and
    // copying History to a new machine must not carry it.
    //
    // The shipped defaults are seeded here rather than defaulted in code, so
    // that an empty table means the Operator removed everything and gets
    // exactly that — not a silent restoration of the defaults on next start.
    r#"
    CREATE TABLE watchlist (
        id    TEXT PRIMARY KEY NOT NULL,
        name  TEXT NOT NULL,
        kind  TEXT NOT NULL CHECK (kind IN ('process', 'browserMeetings'))
    ) STRICT;

    INSERT INTO watchlist (id, name, kind) VALUES
        ('us.zoom.xos',                'Zoom',            'process'),
        ('com.microsoft.teams2',       'Microsoft Teams', 'process'),
        ('com.tencent.meeting',        'VooV Meeting',    'process'),
        ('com.tencent.tencentmeeting', '腾讯会议',         'process'),
        ('browser-meetings',           'Browser Meetings','browserMeetings');
    "#,
    // 6 — what the calendar knew (ADR-0036). The title already rides the
    // Meeting; these are the two facts that would otherwise be lost: which
    // event it was, and who was invited. Attendees are *stored, not
    // applied* — they become Speaker-naming suggestions in M3, and turning
    // an invitation into an attribution before Diarization exists would be
    // inventing who spoke.
    r#"
    ALTER TABLE meetings ADD COLUMN calendar_event_id TEXT;
    ALTER TABLE meetings ADD COLUMN calendar_attendees TEXT;
    "#,
];

/// Applies every migration the database has not seen yet.
pub fn migrate(connection: &mut Connection) -> rusqlite::Result<()> {
    let applied: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let applied = applied as usize;

    for (index, migration) in MIGRATIONS.iter().enumerate().skip(applied) {
        let transaction = connection.transaction()?;
        transaction.execute_batch(migration)?;
        // PRAGMA does not accept a bound parameter.
        transaction.pragma_update(None, "user_version", (index + 1) as i64)?;
        transaction.commit()?;
    }
    Ok(())
}

/// Connection settings every handle needs.
pub fn configure(connection: &Connection) -> rusqlite::Result<()> {
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_apply_and_are_idempotent() {
        let mut connection = Connection::open_in_memory().expect("open");
        configure(&connection).expect("configure");
        migrate(&mut connection).expect("migrate");
        migrate(&mut connection).expect("migrate again");

        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("user_version");
        assert_eq!(version as usize, MIGRATIONS.len());
    }

    #[test]
    fn full_text_search_is_available() {
        // FTS5 is not optional for us: History search is a headline story.
        let connection = Connection::open_in_memory().expect("open");
        connection
            .execute_batch("CREATE VIRTUAL TABLE probe USING fts5(body);")
            .expect("this build of SQLite must have FTS5");
    }

    #[test]
    fn the_channel_enum_is_enforced_by_the_database() {
        let mut connection = Connection::open_in_memory().expect("open");
        configure(&connection).expect("configure");
        migrate(&mut connection).expect("migrate");
        connection
            .execute(
                "INSERT INTO meetings (id, started_at, created_at, updated_at)
                 VALUES ('m', 'now', 'now', 'now')",
                [],
            )
            .expect("insert meeting");

        let result = connection.execute(
            "INSERT INTO transcript_segments (id, meeting_id, sequence, channel, start_ms, end_ms, text)
             VALUES ('s', 'm', 0, 'telepathy', 0, 1, 'hello')",
            [],
        );
        assert!(result.is_err(), "an invalid channel must be rejected");
    }

    #[test]
    fn deleting_a_meeting_takes_its_segments_with_it() {
        let mut connection = Connection::open_in_memory().expect("open");
        configure(&connection).expect("configure");
        migrate(&mut connection).expect("migrate");
        connection
            .execute(
                "INSERT INTO meetings (id, started_at, created_at, updated_at)
                 VALUES ('m', 'now', 'now', 'now')",
                [],
            )
            .expect("insert meeting");
        connection
            .execute(
                "INSERT INTO transcript_segments (id, meeting_id, sequence, channel, start_ms, end_ms, text)
                 VALUES ('s', 'm', 0, 'mic', 0, 1, 'hello')",
                [],
            )
            .expect("insert segment");

        connection
            .execute("DELETE FROM meetings WHERE id = 'm'", [])
            .expect("delete meeting");
        let remaining: i64 = connection
            .query_row("SELECT count(*) FROM transcript_segments", [], |row| {
                row.get(0)
            })
            .expect("count");
        assert_eq!(remaining, 0, "whole-Meeting delete must be complete");
    }
}
