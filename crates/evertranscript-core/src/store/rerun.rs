//! The bulk re-run a model change starts.
//!
//! A model change clears every Voiceprint (ADR-0037, migration 11) and this
//! is what earns them back: every Meeting with Kept Audio walks through
//! Diarization again, oldest first, and what the Operator already told the
//! product is relearned in the new model's vector space.
//!
//! **Oldest first is not cosmetic.** A named Speaker gets its new Voiceprint
//! from the Meetings it was corrected in, so a Meeting re-run after those
//! can recognize it by voice rather than only by correction. Newest-first
//! would reach every Meeting eventually and recognize almost nothing on the
//! way, which is the same end state arrived at looking broken.
//!
//! The work itself lives in [`super::diarize_queue`], which already outlives
//! the process. What is kept here is only what the queue cannot say: which
//! model the backlog is for, how big it was, and whether the Operator
//! stopped it.

use anyhow::Result;
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use rusqlite::params;

use super::diarize_queue;

/// A bulk re-run, as far as anyone outside needs to know about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rerun {
    /// The embedding whose arrival started it.
    pub model: String,
    pub model_version: String,
    /// Meetings enqueued when it began.
    pub total: usize,
    /// Meetings still waiting. Zero means finished — or cancelled, which
    /// `cancelled` is what tells apart.
    pub remaining: usize,
    /// Meetings cancelling threw away. Without it, emptying the queue would
    /// make `done` jump to `total` and report a stopped re-run as a finished
    /// one.
    pub abandoned: usize,
    pub cancelled: bool,
}

impl Rerun {
    /// Meetings already walked.
    ///
    /// Saturating because the numbers come from different places: the total
    /// was written when the backlog was enqueued and the remainder is
    /// counted now, so a Meeting deleted mid-run shrinks the queue without
    /// shrinking the total. Clamping at zero is the honest reading of that,
    /// and a panic is not.
    pub fn done(&self) -> usize {
        self.total
            .saturating_sub(self.remaining)
            .saturating_sub(self.abandoned)
    }

    /// Whether there is still work owed.
    pub fn running(&self) -> bool {
        !self.cancelled && self.remaining > 0
    }
}

/// Starts a re-run if the embedding is one History has not been walked with.
/// Answers how many Meetings were enqueued, or `None` if nothing was owed.
///
/// Called at every start, and idempotent by construction: the stored model
/// identity is both the trigger and the guard. A Core restarted halfway
/// through a backlog sees its own model recorded, does nothing, and lets the
/// queue it left behind carry on — which is what makes "quitting mid-run and
/// restarting resumes rather than restarts" a property of the shape rather
/// than of a flag somebody has to clear.
///
/// A Meeting with no Kept Audio is not enqueued. There is nothing to listen
/// to, so it keeps the attributions it has (ADR-0035): the words, the
/// corrections and the names all stand, and only recognition of the voices
/// in it is gone, which is what a model change costs.
pub fn begin_if_the_model_changed(
    connection: &Connection,
    model: &str,
    model_version: &str,
) -> Result<Option<usize>> {
    let current: Option<(String, String)> = connection
        .query_row(
            "SELECT model, model_version FROM diarize_rerun WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if current
        .as_ref()
        .is_some_and(|(stored_model, stored_version)| {
            stored_model == model && stored_version == model_version
        })
    {
        return Ok(None);
    }

    // Oldest first, and `id` behind `started_at` so two Meetings that began
    // in the same second still have an order — `Uuid::now_v7` makes the id
    // itself chronological, so the tiebreak agrees with the clock rather
    // than fighting it.
    let mut statement = connection.prepare(
        "SELECT id FROM meetings
          WHERE audio_path IS NOT NULL
          ORDER BY started_at, id",
    )?;
    let meetings: Vec<String> = statement
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    drop(statement);

    let mut total = 0;
    for meeting_id in &meetings {
        // At the back, and refused where the Meeting is already in line: one
        // that just ended is ahead of this and stays there.
        if diarize_queue::enqueue(connection, meeting_id, diarize_queue::Priority::Back)? {
            total += 1;
        }
    }

    connection.execute(
        "INSERT INTO diarize_rerun
              (id, model, model_version, total, cancelled, abandoned, started_at)
              VALUES (1, ?1, ?2, ?3, 0, 0, ?4)
         ON CONFLICT(id) DO UPDATE SET
              model = ?1, model_version = ?2, total = ?3,
              cancelled = 0, abandoned = 0, started_at = ?4",
        params![model, model_version, total as i64, super::now_rfc3339()],
    )?;

    Ok((total > 0).then_some(total))
}

/// What the re-run is doing, if one has ever run.
pub fn state(connection: &Connection) -> Result<Option<Rerun>> {
    let row: Option<(String, String, i64, i64, bool)> = connection
        .query_row(
            "SELECT model, model_version, total, abandoned, cancelled \
               FROM diarize_rerun WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?;
    let Some((model, model_version, total, abandoned, cancelled)) = row else {
        return Ok(None);
    };
    Ok(Some(Rerun {
        model,
        model_version,
        total: total.max(0) as usize,
        remaining: diarize_queue::backlog(connection)?,
        abandoned: abandoned.max(0) as usize,
        cancelled,
    }))
}

/// Stops the re-run. Answers how many Meetings it gave up on.
///
/// Every Meeting already walked keeps what that walk concluded: this empties
/// the line, it does not undo attribution. The mark outlives the emptied
/// queue because the next start would otherwise find a drained backlog for
/// the current model and read it as finished — or, worse, find no row and
/// begin the whole thing again.
pub fn cancel(connection: &Connection) -> Result<usize> {
    let dropped = diarize_queue::clear_backlog(connection)?;
    // Recorded, not just counted out of the queue: emptying the line would
    // otherwise make done — total minus remaining — jump to total, and an
    // Operator who stopped a re-run at 1 of 40 would be told all forty had
    // been walked.
    connection.execute(
        "UPDATE diarize_rerun SET cancelled = 1, abandoned = abandoned + ?1 WHERE id = 1",
        params![dropped as i64],
    )?;
    Ok(dropped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let mut connection = Connection::open_in_memory().expect("open");
        crate::store::schema::configure(&connection).expect("configure");
        crate::store::schema::migrate(&mut connection).expect("migrate");
        connection
    }

    /// `started_at` out of order on purpose: the walk follows the clock, not
    /// the insertion order.
    fn meetings(connection: &Connection, entries: &[(&str, &str, Option<&str>)]) {
        for (id, started_at, audio) in entries {
            connection
                .execute(
                    "INSERT INTO meetings (id, started_at, audio_path, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, 'now', 'now')",
                    params![id, started_at, audio],
                )
                .expect("meeting");
        }
    }

    #[test]
    fn a_new_model_enqueues_every_meeting_that_has_audio_oldest_first() {
        let connection = db();
        meetings(
            &connection,
            &[
                ("c", "2024-03-01T00:00:00Z", Some("c.wav")),
                ("a", "2024-01-01T00:00:00Z", Some("a.wav")),
                ("b", "2024-02-01T00:00:00Z", Some("b.wav")),
                ("silent", "2024-02-15T00:00:00Z", None),
            ],
        );

        assert_eq!(
            begin_if_the_model_changed(&connection, "redimnet2", "1").expect("begin"),
            Some(3),
            "the Meeting with no Kept Audio is not work"
        );
        assert_eq!(
            diarize_queue::list(&connection).expect("list"),
            ["a", "b", "c"],
            "oldest first, so a named voice is relearned before the Meetings that need it"
        );
    }

    #[test]
    fn the_same_model_twice_does_not_enqueue_history_again() {
        // The restart case. A Core killed halfway through comes back to a
        // queue that still holds what it owes, and must not pile all of
        // History on top of it.
        let connection = db();
        meetings(
            &connection,
            &[
                ("a", "2024-01-01T00:00:00Z", Some("a.wav")),
                ("b", "2024-02-01T00:00:00Z", Some("b.wav")),
            ],
        );
        begin_if_the_model_changed(&connection, "redimnet2", "1").expect("begin");
        diarize_queue::finish(&connection, "a").expect("worked one");

        assert_eq!(
            begin_if_the_model_changed(&connection, "redimnet2", "1").expect("restart"),
            None
        );
        assert_eq!(diarize_queue::list(&connection).expect("list"), ["b"]);

        let state = state(&connection).expect("state").expect("a re-run");
        assert_eq!((state.total, state.remaining, state.done()), (2, 1, 1));
        assert!(state.running());
    }

    #[test]
    fn a_further_model_change_starts_a_fresh_re_run() {
        let connection = db();
        meetings(&connection, &[("a", "2024-01-01T00:00:00Z", Some("a.wav"))]);
        begin_if_the_model_changed(&connection, "redimnet2", "1").expect("begin");
        diarize_queue::finish(&connection, "a").expect("worked it");

        assert_eq!(
            begin_if_the_model_changed(&connection, "redimnet3", "1").expect("newer model"),
            Some(1),
            "a different embedding is a different vector space, so History owes another walk"
        );
        let state = state(&connection).expect("state").expect("a re-run");
        assert_eq!(state.model, "redimnet3");
        assert_eq!((state.total, state.remaining), (1, 1));
    }

    #[test]
    fn cancelling_empties_the_backlog_and_stays_cancelled_across_a_restart() {
        let connection = db();
        meetings(
            &connection,
            &[
                ("a", "2024-01-01T00:00:00Z", Some("a.wav")),
                ("b", "2024-02-01T00:00:00Z", Some("b.wav")),
                ("just-ended", "2024-03-01T00:00:00Z", Some("j.wav")),
            ],
        );
        begin_if_the_model_changed(&connection, "redimnet2", "1").expect("begin");
        diarize_queue::enqueue(&connection, "just-ended", diarize_queue::Priority::Front)
            .expect("somebody is waiting for this one");

        assert_eq!(cancel(&connection).expect("cancel"), 2);
        assert_eq!(
            diarize_queue::list(&connection).expect("list"),
            ["just-ended"],
            "cancelling the backlog does not cancel work somebody asked for"
        );

        assert_eq!(
            begin_if_the_model_changed(&connection, "redimnet2", "1").expect("restart"),
            None,
            "a cancelled re-run stays cancelled — otherwise 'cancel' means 'until next launch'"
        );
        let state = state(&connection).expect("state").expect("a re-run");
        assert!(state.cancelled && !state.running());
    }

    #[test]
    fn a_cancelled_re_run_does_not_report_itself_as_finished() {
        // Emptying the queue makes remaining zero, and done is total minus
        // remaining — so without the abandoned count, stopping a re-run at
        // one of three tells the Operator all three were walked. That is the
        // one thing the number exists to prevent.
        let connection = db();
        meetings(
            &connection,
            &[
                ("a", "2024-01-01T00:00:00Z", Some("a.wav")),
                ("b", "2024-02-01T00:00:00Z", Some("b.wav")),
                ("c", "2024-03-01T00:00:00Z", Some("c.wav")),
            ],
        );
        begin_if_the_model_changed(&connection, "redimnet2", "1").expect("begin");
        diarize_queue::finish(&connection, "a").expect("walked one");
        cancel(&connection).expect("cancel");

        let state = state(&connection).expect("state").expect("a re-run");
        assert_eq!(
            (state.done(), state.total, state.abandoned),
            (1, 3, 2),
            "one walked, two given up on"
        );
    }
}
