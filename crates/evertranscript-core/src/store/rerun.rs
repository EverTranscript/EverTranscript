//! The bulk re-run a model change owes, **written and inactive**.
//!
//! Ticket 12. A model change clears every Voiceprint (ticket 05's
//! [`super::schema::PENDING_MODEL_CHANGE_WIPE`]) and this is what earns them
//! back: every Meeting with Kept Audio walks through Diarization again,
//! oldest first, in the new model's vector space.
//!
//! **Oldest first is not cosmetic.** A named Speaker gets its new Voiceprint
//! from the Meetings it was corrected in, so a Meeting re-run after those can
//! recognize it by voice rather than only by correction. Newest-first reaches
//! every Meeting eventually and recognizes almost nothing on the way, which
//! is the same end state arrived at looking broken.
//!
//! The work itself lives in [`super::diarize_queue`], which already outlives
//! the process, so resume-rather-than-restart is a property of the shape
//! rather than of a flag somebody clears.
//!
//! **Nothing calls any of this, and its tables are not in `MIGRATIONS`**, so
//! every function here fails on a current History by design. Activation is
//! the user's model decision plus ticket 05; see
//! [`super::schema::PENDING_MODEL_CHANGE_RERUN`].

use anyhow::Result;
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use rusqlite::params;

use super::diarize_queue;

/// A bulk re-run, as far as anything outside needs to know about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rerun {
    /// The embedding this backlog is for. Both the trigger and the guard: a
    /// Core restarted halfway through sees its own model recorded and lets
    /// the queue it left behind carry on.
    pub model: String,
    pub model_version: String,
    /// Meetings enqueued when it began.
    pub total: usize,
    /// How many of *its own* Meetings are still in line — not the size of
    /// the queue, which also carries work this re-run never asked for.
    pub remaining: usize,
    /// Meetings cancelling threw away.
    pub abandoned: usize,
    pub cancelled: bool,
}

impl Rerun {
    /// Meetings already walked.
    ///
    /// Saturating because the numbers come from different places: the total
    /// was written when the backlog was enqueued and the remainder is counted
    /// now, so a Meeting deleted mid-run shrinks the queue without shrinking
    /// the total. Clamping at zero is the honest reading of that; a panic is
    /// not.
    pub fn done(&self) -> usize {
        self.total
            .saturating_sub(self.remaining)
            .saturating_sub(self.abandoned)
    }

    /// Whether there is still work owed. False for a re-run that was never
    /// asked for, which is what the first start after this lands records.
    pub fn running(&self) -> bool {
        !self.cancelled && self.remaining > 0
    }
}

/// What the re-run is doing, if this History has ever recorded one.
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
    // Its own Meetings that are still in line, rather than the whole
    // backlog: `Back` is a scheduling class, and the catch-up pass for
    // Meetings that were never diarized uses it too.
    let remaining: i64 = connection.query_row(
        "SELECT COUNT(*) FROM diarize_rerun_backlog backlog \
           JOIN diarize_queue queue ON queue.meeting_id = backlog.meeting_id",
        [],
        |row| row.get(0),
    )?;
    Ok(Some(Rerun {
        model,
        model_version,
        total: total.max(0) as usize,
        remaining: remaining.max(0) as usize,
        abandoned: abandoned.max(0) as usize,
        cancelled,
    }))
}

/// Asks for a re-run outright. Answers how many Meetings were enqueued.
///
/// The entry point for a transition that *knows* History has to be walked —
/// a wipe, or a deliberate re-derivation — and it does not consult the stored
/// identity, so it works on an installation that has never recorded one.
/// [`begin_if_the_model_changed`] is the passive form and refuses exactly
/// that case.
///
/// A Meeting with no Kept Audio is not enqueued: there is nothing to listen
/// to, so it keeps the attributions it has (ADR-0035). The words, the
/// corrections and the names all stand; only recognition of the voices in it
/// is gone, which is what a model change costs.
///
/// A Meeting already in line is left where it is and is **not** counted: one
/// that just ended, or one the Operator asked for, is ahead of this and stays
/// there, so it is not the re-run's to cancel either.
pub fn begin(connection: &Connection, model: &str, model_version: &str) -> Result<usize> {
    let transaction = connection.unchecked_transaction()?;
    // A previous backlog's membership is not this one's.
    transaction.execute("DELETE FROM diarize_rerun_backlog", [])?;

    // Oldest first, with `id` behind `started_at` so two Meetings that began
    // in the same second still have an order — `Uuid::now_v7` makes the id
    // chronological, so the tiebreak agrees with the clock rather than
    // fighting it.
    let meetings: Vec<String> = {
        let mut statement = transaction.prepare(
            "SELECT id FROM meetings WHERE audio_path IS NOT NULL ORDER BY started_at, id",
        )?;
        let rows = statement.query_map([], |row| row.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };

    let mut total = 0;
    for meeting_id in &meetings {
        if diarize_queue::enqueue(&transaction, meeting_id, diarize_queue::Priority::Back)? {
            transaction.execute(
                "INSERT INTO diarize_rerun_backlog (meeting_id) VALUES (?1)",
                params![meeting_id],
            )?;
            total += 1;
        }
    }
    record(&transaction, model, model_version, total)?;
    transaction.commit()?;
    Ok(total)
}

/// Starts a re-run only if this History has been walked with a *different*
/// embedding. Answers how many Meetings were enqueued, or `None` when
/// nothing was owed.
///
/// Safe to call at every start, and the shape is what makes that true: the
/// stored identity is both trigger and guard, so a Core restarted halfway
/// through a backlog does nothing and lets the queue carry on.
///
/// **An absent row is not evidence of a model change.** Every History
/// predating this feature has no row, and reading that as "the model changed"
/// would enqueue all of History on the first start after an ordinary update —
/// a multi-hour re-run of everything, with no swap behind it. The first start
/// therefore records the identity and asks for nothing. A transition that
/// genuinely needs the walk calls [`begin`], which does not consult the row
/// at all.
pub fn begin_if_the_model_changed(
    connection: &Connection,
    model: &str,
    model_version: &str,
) -> Result<Option<usize>> {
    let stored: Option<(String, String)> = connection
        .query_row(
            "SELECT model, model_version FROM diarize_rerun WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    match stored {
        Some((stored_model, stored_version))
            if stored_model == model && stored_version == model_version =>
        {
            Ok(None)
        }
        Some(_) => begin(connection, model, model_version).map(Some),
        None => {
            record(connection, model, model_version, 0)?;
            Ok(None)
        }
    }
}

/// Stops the re-run. Answers how many Meetings it gave up on.
///
/// Every Meeting already walked keeps what that walk concluded: this empties
/// the line, it does not undo attribution. Only the re-run's own Meetings,
/// and only those still waiting at `Back` — one promoted to `Front` because
/// somebody asked for it is no longer this job's to cancel.
///
/// The mark outlives the emptied queue, or the next start would find a
/// drained backlog for the current model and read it as finished.
pub fn cancel(connection: &Connection) -> Result<usize> {
    let transaction = connection.unchecked_transaction()?;
    let dropped = transaction.execute(
        "DELETE FROM diarize_queue \
          WHERE priority = ?1 \
            AND meeting_id IN (SELECT meeting_id FROM diarize_rerun_backlog)",
        params![diarize_queue::Priority::Back as i64],
    )?;
    transaction.execute("DELETE FROM diarize_rerun_backlog", [])?;
    // Recorded, not just counted out of the queue: emptying the line would
    // otherwise make done — total minus remaining — jump to total, and an
    // Operator who stopped a re-run at 1 of 40 would be told all forty had
    // been walked.
    transaction.execute(
        "UPDATE diarize_rerun SET cancelled = 1, abandoned = abandoned + ?1 WHERE id = 1",
        params![dropped as i64],
    )?;
    transaction.commit()?;
    Ok(dropped)
}

/// The one row, written or replaced.
fn record(
    connection: &Connection,
    model: &str,
    model_version: &str,
    total: usize,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO diarize_rerun \
              (id, model, model_version, total, cancelled, abandoned, started_at) \
              VALUES (1, ?1, ?2, ?3, 0, 0, ?4) \
         ON CONFLICT(id) DO UPDATE SET \
              model = ?1, model_version = ?2, total = ?3, \
              cancelled = 0, abandoned = 0, started_at = ?4",
        params![model, model_version, total as i64, super::now_rfc3339()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A History as the current build leaves one, plus ticket 12's tables.
    ///
    /// The second step is what the product does *not* do on its own: the
    /// migration is unregistered, so nothing here is reachable until the
    /// swap it belongs to.
    fn db() -> Connection {
        let mut connection = Connection::open_in_memory().expect("open");
        crate::store::schema::configure(&connection).expect("configure");
        crate::store::schema::migrate(&mut connection).expect("migrate");
        connection
            .execute_batch(crate::store::schema::PENDING_MODEL_CHANGE_RERUN)
            .expect("the pending re-run tables");
        connection
    }

    /// `started_at` out of insertion order on purpose: the walk follows the
    /// clock, not the order the rows were written.
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

    fn history(connection: &Connection) {
        meetings(
            connection,
            &[
                ("c", "2024-03-01T00:00:00Z", Some("c.wav")),
                ("a", "2024-01-01T00:00:00Z", Some("a.wav")),
                ("b", "2024-02-01T00:00:00Z", Some("b.wav")),
                ("silent", "2024-02-15T00:00:00Z", None),
            ],
        );
    }

    #[test]
    fn a_history_that_has_never_recorded_one_has_no_state() {
        let connection = db();
        assert_eq!(state(&connection).expect("state"), None);
    }

    /// The trap this shape exists to avoid.
    ///
    /// Every History predating the feature has no row. Reading that as a
    /// model change would enqueue all of History on the first start after an
    /// ordinary update, with no swap behind it.
    #[test]
    fn the_first_start_records_the_model_and_asks_for_nothing() {
        let connection = db();
        history(&connection);

        assert_eq!(
            begin_if_the_model_changed(&connection, "wespeaker", "2").expect("first start"),
            None,
            "absent metadata is not evidence that the model changed"
        );
        assert!(
            diarize_queue::list(&connection).expect("list").is_empty(),
            "and History is left alone"
        );
        let recorded = state(&connection).expect("state").expect("a row");
        assert_eq!((recorded.total, recorded.remaining), (0, 0));
        assert!(!recorded.running());
    }

    #[test]
    fn the_same_model_again_asks_for_nothing() {
        let connection = db();
        history(&connection);
        begin_if_the_model_changed(&connection, "wespeaker", "2").expect("first start");

        assert_eq!(
            begin_if_the_model_changed(&connection, "wespeaker", "2").expect("restart"),
            None
        );
        assert!(diarize_queue::list(&connection).expect("list").is_empty());
    }

    #[test]
    fn a_changed_model_enqueues_every_meeting_with_audio_oldest_first() {
        let connection = db();
        history(&connection);
        begin_if_the_model_changed(&connection, "wespeaker", "2").expect("first start");

        assert_eq!(
            begin_if_the_model_changed(&connection, "redimnet2-b3", "1").expect("changed"),
            Some(3),
            "the Meeting with no Kept Audio is not work"
        );
        assert_eq!(
            diarize_queue::list(&connection).expect("list"),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    /// A version bump is a model change: same width, different space.
    #[test]
    fn a_changed_version_of_the_same_model_is_a_change() {
        let connection = db();
        history(&connection);
        begin_if_the_model_changed(&connection, "wespeaker", "2").expect("first start");

        assert_eq!(
            begin_if_the_model_changed(&connection, "wespeaker", "3").expect("changed"),
            Some(3)
        );
    }

    #[test]
    fn a_restart_midway_resumes_rather_than_restarting() {
        let connection = db();
        history(&connection);
        begin(&connection, "redimnet2-b3", "1").expect("begin");
        diarize_queue::finish(&connection, "a").expect("walked");

        assert_eq!(
            begin_if_the_model_changed(&connection, "redimnet2-b3", "1").expect("restart"),
            None,
            "the queue it left behind is the resume"
        );
        let resumed = state(&connection).expect("state").expect("a row");
        assert_eq!(
            (resumed.total, resumed.remaining, resumed.done()),
            (3, 2, 1)
        );
        assert!(resumed.running());
    }

    /// An explicit transition does not need a row to have existed.
    #[test]
    fn a_wipe_can_ask_for_a_rerun_on_an_installation_with_no_prior_metadata() {
        let connection = db();
        history(&connection);
        assert_eq!(state(&connection).expect("state"), None, "the premise");

        assert_eq!(begin(&connection, "redimnet2-b3", "1").expect("begin"), 3);
        let asked = state(&connection).expect("state").expect("a row");
        assert_eq!((asked.total, asked.remaining), (3, 3));
        assert!(asked.running());
    }

    #[test]
    fn cancelling_reports_what_it_gave_up_rather_than_what_it_finished() {
        let connection = db();
        history(&connection);
        begin(&connection, "redimnet2-b3", "1").expect("begin");
        diarize_queue::finish(&connection, "a").expect("walked");

        assert_eq!(cancel(&connection).expect("cancel"), 2);
        let stopped = state(&connection).expect("state").expect("a row");
        assert!(stopped.cancelled && !stopped.running());
        assert_eq!(
            (stopped.done(), stopped.abandoned),
            (1, 2),
            "one walked and two given up, not three walked"
        );
    }

    /// Cancelling is not a reason to begin the whole thing again.
    #[test]
    fn a_cancelled_rerun_is_not_restarted_by_the_next_start() {
        let connection = db();
        history(&connection);
        begin(&connection, "redimnet2-b3", "1").expect("begin");
        cancel(&connection).expect("cancel");

        assert_eq!(
            begin_if_the_model_changed(&connection, "redimnet2-b3", "1").expect("next start"),
            None
        );
        assert!(diarize_queue::list(&connection).expect("list").is_empty());
    }

    /// The re-run owns its own Meetings, not the queue.
    ///
    /// `Back` is a scheduling class: production already enqueues there for
    /// Meetings that were never diarized. Counting the whole backlog would
    /// report that catch-up as re-run progress, and cancelling would delete
    /// somebody else's work.
    #[test]
    fn work_the_rerun_never_asked_for_is_neither_counted_nor_cancelled() {
        let connection = db();
        history(&connection);
        begin(&connection, "redimnet2-b3", "1").expect("begin");

        meetings(
            &connection,
            &[("later", "2024-04-01T00:00:00Z", Some("later.wav"))],
        );
        diarize_queue::enqueue(&connection, "later", diarize_queue::Priority::Back)
            .expect("catch-up");
        // A Meeting somebody is waiting for, which was also the re-run's.
        diarize_queue::enqueue(&connection, "b", diarize_queue::Priority::Front)
            .expect("asked for");

        let during = state(&connection).expect("state").expect("a row");
        assert_eq!(during.remaining, 3, "the catch-up Meeting is not its work");

        assert_eq!(
            cancel(&connection).expect("cancel"),
            2,
            "and the promoted Meeting is no longer its work either"
        );
        assert_eq!(
            diarize_queue::list(&connection).expect("list"),
            vec!["b".to_string(), "later".to_string()],
            "what somebody is waiting for, and what the re-run never asked for"
        );
    }
}
