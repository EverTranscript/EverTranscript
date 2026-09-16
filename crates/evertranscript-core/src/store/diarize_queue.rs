//! The Meetings waiting to be diarized.
//!
//! In the record rather than in memory: a model change re-runs all of
//! History, and a Core killed halfway through that has to know what it still
//! owes when it comes back. A queue that lived in a process would leave a
//! half-attributed History behind, which reads as diarization being
//! unreliable rather than as a restart.

use anyhow::Result;
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use rusqlite::params;

/// Where a Meeting enters the line.
///
/// Small-number-first, matching the column's own ordering, so the enum and
/// the `ORDER BY` cannot drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    /// A Meeting that just ended, or one the Operator asked for. Goes ahead
    /// of bulk work whatever the arrival order: somebody is waiting for it.
    Front = 0,
    /// A re-run of History after a model change. The machine has all night.
    Back = 1,
}

impl Priority {
    /// The column's value, read back. The `CHECK` constraint allows only the
    /// two, so anything else is a database written by something that is not
    /// this program, and bulk is the safe reading of it: it waits its turn.
    fn from_column(value: i64) -> Self {
        if value == Priority::Front as i64 {
            Priority::Front
        } else {
            Priority::Back
        }
    }
}

/// Puts a Meeting in line. Answers whether it was added.
///
/// `false` means it was already queued — the caller's cue to say so rather
/// than to report a run that will not happen. A Meeting already in line at
/// `Back` is promoted when it is asked for again at `Front`, because an
/// Operator waiting behind an overnight backlog for a Meeting the backlog
/// happens to contain is the same complaint as being refused.
pub fn enqueue(connection: &Connection, meeting_id: &str, priority: Priority) -> Result<bool> {
    let existing: Option<i64> = connection
        .query_row(
            "SELECT priority FROM diarize_queue WHERE meeting_id = ?1",
            params![meeting_id],
            |row| row.get(0),
        )
        .optional()?;

    match existing {
        Some(current) if current <= priority as i64 => Ok(false),
        Some(_) => {
            connection.execute(
                "UPDATE diarize_queue SET priority = ?2, enqueued_at = ?3 WHERE meeting_id = ?1",
                params![meeting_id, priority as i64, crate::store::now_rfc3339()],
            )?;
            Ok(false)
        }
        None => {
            connection.execute(
                "INSERT INTO diarize_queue (meeting_id, priority, enqueued_at) \
                 VALUES (?1, ?2, ?3)",
                params![meeting_id, priority as i64, crate::store::now_rfc3339()],
            )?;
            Ok(true)
        }
    }
}

/// The Meeting to work on next, without taking it out of the line.
///
/// Left in place on purpose: a Core that crashes mid-run comes back owing
/// the same Meeting. Removed by `finish` once the run is over, whatever its
/// outcome.
///
/// The priority comes back with it because the worker treats the two kinds
/// of work differently: bulk work waits while a Meeting is being recorded,
/// and a just-ended Meeting does not.
pub fn peek(connection: &Connection) -> Result<Option<(String, Priority)>> {
    Ok(connection
        .query_row(
            "SELECT meeting_id, priority FROM diarize_queue \
              ORDER BY priority, enqueued_at LIMIT 1",
            [],
            |row| {
                let id: String = row.get(0)?;
                let priority: i64 = row.get(1)?;
                Ok((id, Priority::from_column(priority)))
            },
        )
        .optional()?)
}

/// How much bulk work is still owed.
pub fn backlog(connection: &Connection) -> Result<usize> {
    Ok(connection.query_row(
        "SELECT COUNT(*) FROM diarize_queue WHERE priority = ?1",
        params![Priority::Back as i64],
        |row| row.get::<_, i64>(0),
    )? as usize)
}

/// Empties the bulk backlog, leaving work somebody is waiting for.
///
/// A cancelled re-run must not take the Meeting that just ended down with
/// it: they are in the same line for scheduling, not because they are the
/// same job.
pub fn clear_backlog(connection: &Connection) -> Result<usize> {
    Ok(connection.execute(
        "DELETE FROM diarize_queue WHERE priority = ?1",
        params![Priority::Back as i64],
    )?)
}

/// Takes a Meeting out of the line.
pub fn finish(connection: &Connection, meeting_id: &str) -> Result<()> {
    connection.execute(
        "DELETE FROM diarize_queue WHERE meeting_id = ?1",
        params![meeting_id],
    )?;
    Ok(())
}

/// Whether a Meeting is already in line.
pub fn holds(connection: &Connection, meeting_id: &str) -> Result<bool> {
    Ok(connection
        .query_row(
            "SELECT 1 FROM diarize_queue WHERE meeting_id = ?1",
            params![meeting_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Everyone in line, in the order they will be worked.
pub fn list(connection: &Connection) -> Result<Vec<String>> {
    let mut statement = connection
        .prepare("SELECT meeting_id FROM diarize_queue ORDER BY priority, enqueued_at")?;
    let rows = statement.query_map([], |row| row.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let mut connection = Connection::open_in_memory().expect("open");
        crate::store::schema::configure(&connection).expect("configure");
        crate::store::schema::migrate(&mut connection).expect("migrate");
        for id in ["a", "b", "c"] {
            connection
                .execute(
                    "INSERT INTO meetings (id, started_at, created_at, updated_at) \
                     VALUES (?1, 'now', 'now', 'now')",
                    params![id],
                )
                .expect("meeting");
        }
        connection
    }

    #[test]
    fn a_just_ended_meeting_goes_ahead_of_bulk_work() {
        // The case the queue exists for: a re-run of all of History is
        // already queued when somebody stops a call.
        let connection = db();
        enqueue(&connection, "a", Priority::Back).expect("bulk");
        enqueue(&connection, "b", Priority::Back).expect("bulk");
        enqueue(&connection, "c", Priority::Front).expect("just ended");

        assert_eq!(list(&connection).expect("list"), ["c", "a", "b"]);
    }

    #[test]
    fn the_same_meeting_twice_is_refused_rather_than_queued_twice() {
        let connection = db();
        assert!(enqueue(&connection, "a", Priority::Front).expect("first"));
        assert!(!enqueue(&connection, "a", Priority::Front).expect("second"));
        assert_eq!(list(&connection).expect("list"), ["a"]);
    }

    #[test]
    fn asking_for_a_meeting_the_backlog_holds_moves_it_to_the_front() {
        // Still a refusal — the caller is told no new run was started — but
        // the wait it was told about is the short one.
        let connection = db();
        enqueue(&connection, "a", Priority::Back).expect("bulk");
        enqueue(&connection, "b", Priority::Back).expect("bulk");
        assert!(
            !enqueue(&connection, "b", Priority::Front).expect("asked for"),
            "already in line, so nothing was added"
        );
        assert_eq!(list(&connection).expect("list"), ["b", "a"]);
    }

    #[test]
    fn a_run_that_crashes_is_still_owed_and_a_finished_one_is_not() {
        let connection = db();
        enqueue(&connection, "a", Priority::Front).expect("queue");
        assert_eq!(
            peek(&connection).expect("peek"),
            Some(("a".to_string(), Priority::Front))
        );
        assert_eq!(
            peek(&connection).expect("peek again"),
            Some(("a".to_string(), Priority::Front)),
            "peeking does not consume: a Core that dies mid-run comes back owing it"
        );
        finish(&connection, "a").expect("finish");
        assert_eq!(peek(&connection).expect("peek"), None);
    }

    #[test]
    fn deleting_a_meeting_takes_it_out_of_the_line() {
        let connection = db();
        enqueue(&connection, "a", Priority::Back).expect("queue");
        connection
            .execute("DELETE FROM meetings WHERE id = 'a'", [])
            .expect("delete");
        assert!(list(&connection).expect("list").is_empty());
        assert!(!holds(&connection, "a").expect("holds"));
    }
}
