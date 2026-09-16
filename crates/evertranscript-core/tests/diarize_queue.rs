//! Diarization waits its turn, and remembers it across a restart.
//!
//! M3's policy was to refuse an overlapping run, which was right while the
//! only producer was a Meeting ending. A model change re-runs all of
//! History, and under that policy every Meeting that ended during the re-run
//! was dropped with only a log line about it. These are the properties the
//! queue replaced it with.
//!
//! No worker is spawned here on purpose. What is under test is the line
//! itself — who is in it, in what order, and what a second request is told —
//! and a worker draining an empty-model queue in microseconds would make
//! every one of those assertions a race.

use std::path::Path;
use std::sync::Arc;

use evertranscript_core::server::Core;
use evertranscript_core::store::diarize_queue::Priority;

async fn new_core(history_dir: &Path) -> Arc<Core> {
    Core::with_history_dir_acknowledged(history_dir.to_path_buf()).expect("core")
}

/// A finished Meeting with no audio: enough to be queued, and it runs to
/// `Ok(0)` immediately if anything ever does pick it up.
async fn finished_meeting(core: &Arc<Core>) -> String {
    let meeting = core.start_meeting(None, None).await.expect("start");
    core.stop_meeting().await.expect("stop");
    meeting.id
}

#[tokio::test]
async fn a_meeting_that_ends_during_a_backlog_goes_ahead_of_it() {
    // The case the queue exists for. Under refuse-don't-queue this Meeting
    // was told a run had started and then silently dropped.
    let dir = tempfile::tempdir().expect("tempdir");
    let history_dir = dir.path().join("History");
    let core = new_core(&history_dir).await;

    // Old Meetings, already attributed once and out of the line, put back in
    // at the back — what a re-run of History after a model change does.
    let mut backlog = Vec::new();
    for _ in 0..3 {
        let id = finished_meeting(&core).await;
        core.diarize_cancel(&id).await;
        core.enqueue_diarization(&id, Priority::Back)
            .await
            .expect("queue bulk");
        backlog.push(id);
    }

    // `stop_meeting` queues at Front by itself — this is the real path, not
    // a hand-made enqueue.
    let just_ended = finished_meeting(&core).await;

    let status = core.diarize_status().await;
    assert_eq!(
        status.queued.first(),
        Some(&just_ended),
        "the Meeting somebody is waiting for goes first: {:?}",
        status.queued
    );
    assert_eq!(
        &status.queued[1..],
        backlog.as_slice(),
        "and the backlog keeps its own order behind it"
    );
}

#[tokio::test]
async fn a_meeting_already_in_line_is_refused_rather_than_queued_twice() {
    let dir = tempfile::tempdir().expect("tempdir");
    let history_dir = dir.path().join("History");
    let core = new_core(&history_dir).await;

    let id = finished_meeting(&core).await;
    assert!(
        core.diarization_holds(&id).await.expect("holds"),
        "stopping put it in line"
    );
    assert!(
        !core
            .enqueue_diarization(&id, Priority::Front)
            .await
            .expect("second request"),
        "and asking again is a refusal the caller can report, not a second run"
    );
    assert_eq!(core.diarize_status().await.queued, [id]);
}

#[tokio::test]
async fn the_queue_survives_a_restart() {
    // In the record rather than in memory: a Core killed mid-backlog that
    // forgot what it owed would leave a half-attributed History, which reads
    // as diarization being unreliable rather than as a restart.
    let dir = tempfile::tempdir().expect("tempdir");
    let history_dir = dir.path().join("History");

    let owed = {
        let core = new_core(&history_dir).await;
        let first = finished_meeting(&core).await;
        let second = finished_meeting(&core).await;
        core.enqueue_diarization(&second, Priority::Back)
            .await
            .expect("queue");
        let queued = core.diarize_status().await.queued;
        assert_eq!(queued.len(), 2);
        drop(core);
        (first, queued)
    };

    let restarted = new_core(&history_dir).await;
    assert_eq!(
        restarted.diarize_status().await.queued,
        owed.1,
        "the same Meetings, in the same order"
    );
    assert!(
        restarted.diarization_holds(&owed.0).await.expect("holds"),
        "including the one that was next"
    );
}

#[tokio::test]
async fn cancelling_a_waiting_meeting_takes_it_out_of_the_line() {
    // Otherwise "cancel" means "cancel, then run anyway in four minutes".
    let dir = tempfile::tempdir().expect("tempdir");
    let history_dir = dir.path().join("History");
    let core = new_core(&history_dir).await;

    let waiting = finished_meeting(&core).await;
    let other = finished_meeting(&core).await;
    assert_eq!(core.diarize_status().await.queued.len(), 2);

    let after = core.diarize_cancel(&waiting).await;
    assert_eq!(after.queued, [other], "only the cancelled one left");
}

#[tokio::test]
async fn a_deleted_meeting_leaves_the_line() {
    // Work owed on a Meeting that no longer exists is not work.
    let dir = tempfile::tempdir().expect("tempdir");
    let history_dir = dir.path().join("History");
    let core = new_core(&history_dir).await;

    let id = finished_meeting(&core).await;
    assert_eq!(
        core.diarize_status().await.queued,
        std::slice::from_ref(&id)
    );

    core.delete_meeting(&id).await.expect("delete");
    assert!(
        core.diarize_status().await.queued.is_empty(),
        "deleting the Meeting took it out of the line"
    );
}

/// A finished Meeting the record believes has Kept Audio.
///
/// The bytes are never opened here — no worker is spawned — but the path is
/// what makes a Meeting re-runnable, so a Meeting without one is a different
/// test's subject.
async fn meeting_with_kept_audio(core: &Arc<Core>) -> String {
    let id = finished_meeting(core).await;
    let target = id.clone();
    core.store()
        .write(move |connection| {
            evertranscript_core::store::meetings::set_audio_path(
                connection,
                &target,
                ".data/audio/kept.mp3",
            )
        })
        .await
        .expect("audio path");
    // Out of the line: `stop_meeting` queued it at the front, and what is
    // under test is the backlog a model change puts it in.
    core.diarize_cancel(&id).await;
    id
}

#[tokio::test]
async fn a_model_change_puts_all_of_history_in_line_and_says_so() {
    // Ticket 12's first move. An unannounced multi-hour job that
    // reprocesses everything is indistinguishable, from outside, from the
    // product misbehaving — so the queue is only half of it and the status
    // is the other half.
    let dir = tempfile::tempdir().expect("tempdir");
    let core = new_core(&dir.path().join("History")).await;

    let mut history = Vec::new();
    for _ in 0..3 {
        history.push(meeting_with_kept_audio(&core).await);
    }
    let silent = finished_meeting(&core).await;
    core.diarize_cancel(&silent).await;

    core.rerun_history_if_the_model_changed().await;

    let status = core.diarize_status().await;
    assert_eq!(
        status.queued, history,
        "every Meeting with Kept Audio, oldest first, and nothing else"
    );
    let rerun = status.rerun.expect("an Operator is owed an explanation");
    assert_eq!((rerun.done, rerun.total), (0, 3));
    assert!(!rerun.paused && !rerun.cancelled);
    assert!(
        !status.queued.contains(&silent),
        "a Meeting with no Kept Audio cannot be re-run, so it keeps the attributions it has"
    );
}

#[tokio::test]
async fn restarting_resumes_the_backlog_rather_than_starting_it_again() {
    // A Core killed mid-backlog comes back owing what is left, not all of
    // History a second time. The model identity in the record is what makes
    // that structural rather than a flag somebody has to remember to clear.
    let dir = tempfile::tempdir().expect("tempdir");
    let history_dir = dir.path().join("History");
    let core = new_core(&history_dir).await;
    for _ in 0..3 {
        meeting_with_kept_audio(&core).await;
    }
    core.rerun_history_if_the_model_changed().await;

    // One walked, then the lights go out.
    let first = core.diarize_status().await.queued[0].clone();
    core.diarize_cancel(&first).await;
    drop(core);

    let restarted = new_core(&history_dir).await;
    restarted.rerun_history_if_the_model_changed().await;

    let status = restarted.diarize_status().await;
    assert_eq!(status.queued.len(), 2, "it owes what it had left, not four");
    assert!(!status.queued.contains(&first), "and not the one it walked");
    let rerun = status.rerun.expect("still owed");
    assert_eq!((rerun.done, rerun.total), (1, 3));
}

#[tokio::test]
async fn the_backlog_stands_down_while_a_meeting_records() {
    // Hours of two neural models on the machine the Operator is recording
    // with is the one thing the re-run must never be. Reported as paused
    // rather than idle: there is work owed and a recording in the way.
    let dir = tempfile::tempdir().expect("tempdir");
    let core = new_core(&dir.path().join("History")).await;
    for _ in 0..2 {
        meeting_with_kept_audio(&core).await;
    }
    core.rerun_history_if_the_model_changed().await;
    assert!(!core.diarize_status().await.rerun.expect("owed").paused);

    core.start_meeting(None, None).await.expect("start");
    assert!(core.is_recording().await);
    assert!(
        core.diarize_status().await.rerun.expect("owed").paused,
        "the backlog waits while a Meeting records"
    );

    core.stop_meeting().await.expect("stop");
    assert!(
        !core.diarize_status().await.rerun.expect("owed").paused,
        "and picks up again afterwards"
    );
}

#[tokio::test]
async fn cancelling_the_re_run_stops_it_without_touching_what_it_finished() {
    // A job the Operator cannot stop is an unaccountable use of their
    // machine. Cancelling empties the line; it does not undo attribution,
    // and it does not take down the Meeting somebody is waiting for.
    let dir = tempfile::tempdir().expect("tempdir");
    let history_dir = dir.path().join("History");
    let core = new_core(&history_dir).await;
    for _ in 0..3 {
        meeting_with_kept_audio(&core).await;
    }
    core.rerun_history_if_the_model_changed().await;
    let walked = core.diarize_status().await.queued[0].clone();
    core.diarize_cancel(&walked).await;

    // A Meeting ends while the backlog is still owed.
    let just_ended = finished_meeting(&core).await;

    let status = core.diarize_rerun_cancel().await;
    assert_eq!(
        status.queued,
        std::slice::from_ref(&just_ended),
        "the backlog is gone and the Meeting somebody is waiting for is not"
    );
    let rerun = status.rerun.expect("cancellation is reported, not silent");
    assert!(rerun.cancelled);
    assert_eq!((rerun.done, rerun.total), (1, 3));

    // And it stays cancelled: a re-run that came back on the next launch
    // would make "cancel" mean "until you quit".
    drop(core);
    let restarted = new_core(&history_dir).await;
    restarted.rerun_history_if_the_model_changed().await;
    let status = restarted.diarize_status().await;
    assert_eq!(
        status.queued,
        [just_ended],
        "no backlog came back — and the Meeting somebody is waiting for \
         survived the restart, as the queue has always promised"
    );
    assert!(status.rerun.expect("still says so").cancelled);
}
