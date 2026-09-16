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

use std::path::PathBuf;
use std::sync::Arc;

use evertranscript_core::server::Core;
use evertranscript_core::store::diarize_queue::Priority;

async fn core(history_dir: &PathBuf) -> Arc<Core> {
    Core::with_history_dir_acknowledged(history_dir.clone()).expect("core")
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
    let core = core(&history_dir).await;

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
    let core = core(&history_dir).await;

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
        let core = core(&history_dir).await;
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

    let restarted = core(&history_dir).await;
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
    let core = core(&history_dir).await;

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
    let core = core(&history_dir).await;

    let id = finished_meeting(&core).await;
    assert_eq!(core.diarize_status().await.queued, [id.clone()]);

    core.delete_meeting(&id).await.expect("delete");
    assert!(
        core.diarize_status().await.queued.is_empty(),
        "deleting the Meeting took it out of the line"
    );
}
