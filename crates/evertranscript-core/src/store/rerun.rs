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
    /// Meetings this backlog owns. Set when it began, and carried across a
    /// replacement for the ones still in line — see [`begin`].
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

    /// Whether a backlog was ever asked for.
    ///
    /// The first start writes this row down to record which embedding the
    /// History is in, and asks for nothing — so a row alone is not a re-run.
    /// Reporting that baseline as a re-run of zero Meetings would put a
    /// backlog in front of every Operator who had never triggered one.
    /// `cancelled` counts, so stopping a backlog that had nothing left in it
    /// is still a re-run that was stopped rather than one that never existed.
    pub fn requested(&self) -> bool {
        self.total > 0 || self.abandoned > 0 || self.cancelled
    }
}

/// What the re-run is doing, if this History has ever recorded one.
pub fn state(connection: &Connection) -> Result<Option<Rerun>> {
    // The tables are not in `MIGRATIONS`, so every History in the field is
    // missing them. Asked for by name rather than inferred from the error
    // text of the query below, so that a genuinely broken read — a corrupt
    // page, a locked file — is still an error and not reported as "no
    // re-run". If this table is here and the backlog one is not, the count
    // below fails and that failure is propagated, which is the right answer
    // for a half-installed schema.
    let installed: Option<i64> = connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'diarize_rerun'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if installed.is_none() {
        return Ok(None);
    }
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
/// A Meeting already in line that this backlog does **not** already own is
/// left where it is and is not counted: one that just ended, or one the
/// Operator asked for, is ahead of this and stays there, so it is not the
/// re-run's to cancel either.
///
/// **Replacing a backlog carries the work it still owns.** A second model
/// change mid-walk finds its own Meetings already queued, and `enqueue`
/// answers `false` for every one of them — it has nothing to add. Dropping
/// membership on that answer would leave those Meetings queued and ownerless:
/// `total` would be the handful that happened to have finished, `remaining`
/// zero, and cancelling would empty nothing while the machine kept working.
/// So ownership is reconciled rather than rebuilt, and only Meetings this
/// backlog neither owns nor enqueued are left alone.
pub fn begin(connection: &Connection, model: &str, model_version: &str) -> Result<usize> {
    let transaction = connection.unchecked_transaction()?;

    // Read before anything moves: what the previous backlog still owns.
    let owned: std::collections::BTreeSet<String> = {
        let mut statement = transaction.prepare("SELECT meeting_id FROM diarize_rerun_backlog")?;
        let rows = statement.query_map([], |row| row.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };

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

    // A new model needs its own oldest-first walk, and a row that survived
    // the previous backlog still carries that backlog's `enqueued_at` — so
    // re-enqueuing only the Meetings that had finished would leave them
    // behind the ones that had not, whatever their dates. The re-run's own
    // `Back` rows go back in the line below, in order. A Meeting somebody
    // promoted to `Front` is left exactly where it is, and work this backlog
    // never owned keeps its place ahead of the new one.
    for meeting_id in &owned {
        transaction.execute(
            "DELETE FROM diarize_queue WHERE meeting_id = ?1 AND priority = ?2",
            params![meeting_id, diarize_queue::Priority::Back as i64],
        )?;
    }

    let mut mine: Vec<&String> = Vec::new();
    for meeting_id in &meetings {
        let joined =
            diarize_queue::enqueue(&transaction, meeting_id, diarize_queue::Priority::Back)?;
        // Newly in line, or already in line and already this backlog's.
        if joined || owned.contains(meeting_id) {
            mine.push(meeting_id);
        }
    }

    transaction.execute("DELETE FROM diarize_rerun_backlog", [])?;
    for meeting_id in &mine {
        transaction.execute(
            "INSERT INTO diarize_rerun_backlog (meeting_id) VALUES (?1)",
            params![meeting_id],
        )?;
    }
    let total = mine.len();
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

/// Whether this re-run owns a Meeting that is still waiting as bulk work.
///
/// [`cancel`]'s rule asked about one Meeting. The caller stopping a running
/// job uses it so that the job is stopped exactly when cancelling would have
/// taken its row out anyway: a Meeting somebody promoted to `Front` keeps
/// running, because it is no longer this job's to cancel, and a Meeting the
/// re-run never asked for was never its business.
pub fn owns_bulk_work(connection: &Connection, meeting_id: &str) -> Result<bool> {
    Ok(connection
        .query_row(
            "SELECT 1 FROM diarize_rerun_backlog backlog \
               JOIN diarize_queue queue ON queue.meeting_id = backlog.meeting_id \
              WHERE backlog.meeting_id = ?1 AND queue.priority = ?2",
            params![meeting_id, diarize_queue::Priority::Back as i64],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Stops the re-run. Answers how many Meetings it gave up on.
///
/// Every Meeting already walked keeps what that walk concluded: this empties
/// the line, it does not undo attribution. Only the re-run's own Meetings,
/// and only those still waiting at `Back` — one promoted to `Front` because
/// somebody asked for it is no longer this job's to cancel.
///
/// **A Meeting it cannot cancel it also does not disown.** Promotion is not
/// completion: that Meeting is still in line and will still be walked, so it
/// stays counted in `remaining` until it is. Clearing membership wholesale
/// here would drop it out of `remaining`, and `done` — total minus remaining
/// minus abandoned — would report it as walked the moment it was promoted.
///
/// The mark outlives the emptied queue, or the next start would find a
/// drained backlog for the current model and read it as finished.
pub fn cancel(connection: &Connection) -> Result<usize> {
    let transaction = connection.unchecked_transaction()?;
    // Named before they are deleted, so membership can be given up for
    // exactly the rows the queue gave up and no others — and each one says
    // whether it has already been walked *by this re-run*, because a row
    // still in the queue is not evidence that it has not been.
    //
    // A run commits its attribution and its `diarized_at` in one transaction
    // on the store's writer thread, and the worker takes the queue row out
    // afterwards in a second write. Between those two a cancellation can
    // land, and counting what it finds as abandoned would report a Meeting
    // that was walked as one that was given up on. `diarized_at` compared
    // against this re-run's own start is what tells them apart; `julianday`
    // rather than a string compare because both stamps are local-time RFC
    // 3339 and two offsets do not sort.
    let giving_up: Vec<(String, bool)> = {
        let mut statement = transaction.prepare(
            "SELECT backlog.meeting_id, \
                    IFNULL(julianday(meeting.diarized_at) >= julianday(rerun.started_at), 0) \
               FROM diarize_rerun_backlog backlog \
               JOIN diarize_queue queue ON queue.meeting_id = backlog.meeting_id \
               JOIN meetings meeting ON meeting.id = backlog.meeting_id \
               JOIN diarize_rerun rerun ON rerun.id = 1 \
              WHERE queue.priority = ?1",
        )?;
        let rows = statement.query_map(params![diarize_queue::Priority::Back as i64], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for (meeting_id, _) in &giving_up {
        transaction.execute(
            "DELETE FROM diarize_queue WHERE meeting_id = ?1",
            params![meeting_id],
        )?;
        transaction.execute(
            "DELETE FROM diarize_rerun_backlog WHERE meeting_id = ?1",
            params![meeting_id],
        )?;
    }
    // A Meeting that was walked leaves both tables like the rest, which
    // drops it out of `remaining` — so `done`, total minus remaining minus
    // abandoned, counts it walked. Only the ones actually given up on are
    // added to `abandoned`.
    let dropped = giving_up.iter().filter(|(_, walked)| !walked).count();
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

        // And promotion is not completion. `b` was not cancelled because it
        // is no longer this job's to cancel, but it has not been walked
        // either, so it stays owed until it is.
        let stopped = state(&connection).expect("state").expect("a row");
        assert_eq!(
            (stopped.remaining, stopped.abandoned, stopped.done()),
            (1, 2, 0),
            "nothing was processed, so nothing is done"
        );
        diarize_queue::finish(&connection, "b").expect("walked");
        assert_eq!(state(&connection).expect("state").expect("a row").done(), 1);
    }

    /// A second model change part-way through the first one's backlog.
    ///
    /// `enqueue` has nothing to add for Meetings already in line and answers
    /// `false` for every one of them. Rebuilding membership from that answer
    /// would leave them queued and ownerless — `total` the handful that
    /// happened to have finished, `remaining` zero, and cancelling emptying
    /// nothing while the machine kept working.
    #[test]
    fn beginning_again_mid_backlog_keeps_the_work_it_already_owns() {
        let connection = db();
        history(&connection);
        assert_eq!(begin(&connection, "redimnet2-b3", "1").expect("first"), 3);
        diarize_queue::finish(&connection, "a").expect("walked");

        // Two still queued from the first backlog, one to enqueue again.
        assert_eq!(begin(&connection, "redimnet2-b3", "2").expect("second"), 3);
        let again = state(&connection).expect("state").expect("a row");
        assert_eq!((again.total, again.remaining, again.done()), (3, 3, 0));
        assert_eq!(
            diarize_queue::list(&connection).expect("list"),
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
            "the new model gets its own oldest-first walk, not the leftovers \
             of the old one's with the requeued Meeting behind them"
        );

        assert_eq!(
            cancel(&connection).expect("cancel"),
            3,
            "all three are this backlog's to give up"
        );
        assert!(diarize_queue::list(&connection).expect("list").is_empty());
    }
}
