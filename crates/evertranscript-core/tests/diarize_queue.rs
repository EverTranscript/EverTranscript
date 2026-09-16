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

use evertranscript_core::audio::fixture::FixtureSource;
use evertranscript_core::audio::fixture::Step;
use evertranscript_core::server::Core;
use evertranscript_core::store::diarize_queue::Priority;
use evertranscript_protocol::AudioChannel;

/// A Core whose capture is scripted, like every other test file here.
///
/// **Nothing in this file is about capture**, but `Core` falls back to
/// `LiveSource::new()` when nothing installs a factory, so a `meeting/start`
/// here would open a real device to obtain a finished row. That is what Q55
/// found the last time: `STATUS_ACCESS_VIOLATION` on a Windows runner with no
/// audio hardware, green on every machine that has a microphone.
async fn core(history_dir: &Path) -> Arc<Core> {
    let core = Core::with_history_dir_acknowledged(history_dir.to_path_buf()).expect("core");
    core.set_source_factory(Arc::new(|| {
        Box::new(FixtureSource::new(vec![
            Step::audio(AudioChannel::Mic, 400, 0.3),
            Step::audio(AudioChannel::System, 400, -0.3),
        ]))
    }))
    .await;
    core
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
    assert_eq!(
        core.diarize_status().await.queued,
        std::slice::from_ref(&id)
    );
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

// ---------------------------------------------------------------------------
// A recording stands the backlog down. These are the only tests in this file
// that spawn the worker, and they can because the decision under test is made
// before any model is touched: with no models installed a run that is *let*
// through reaches `Ok(Wrote(0))` and takes its Meeting out of the line, so
// "still queued" and "drained" are distinguishable without inference. What
// that cannot reach is the interrupt landing in the middle of an inference
// pass — that needs the ONNX models, so the decision behind it is asserted in
// `server::tests::only_bulk_work_stands_down_for_a_recording` instead.
// ---------------------------------------------------------------------------

/// Runs the worker until `settled` holds, or gives up after `within`.
///
/// A deadline rather than a fixed sleep: the drain cases finish in
/// milliseconds and only the negative case pays the whole wait.
async fn worker_until(
    core: &Arc<Core>,
    within: std::time::Duration,
    settled: impl Fn(&[String]) -> bool,
) -> Vec<String> {
    let shutdown = tokio_util::sync::CancellationToken::new();
    let worker = tokio::spawn(Arc::clone(core).run_diarization_queue(shutdown.clone()));
    let deadline = std::time::Instant::now() + within;
    let mut queued = core.diarize_status().await.queued;
    while !settled(&queued) && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        queued = core.diarize_status().await.queued;
    }
    shutdown.cancel();
    core.diarize_wake().notify_one();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), worker).await;
    queued
}

/// A finished Meeting put back at the back of the line: what a model change's
/// re-run of History, and a previous Core's unfinished catch-up, both look
/// like.
async fn backlogged(core: &Arc<Core>) -> String {
    let id = finished_meeting(core).await;
    core.diarize_cancel(&id).await;
    core.enqueue_diarization(&id, Priority::Back)
        .await
        .expect("queue bulk");
    id
}

#[tokio::test]
async fn a_recording_stands_the_backlog_down_and_leaves_it_owed() {
    // Two neural models and a capture pass want the same machine. The
    // overnight re-run is the one with nobody waiting for it, so it is the one
    // that waits — and waiting must not mean forgetting, or a model change
    // would quietly skip every Meeting that overlapped a call.
    let dir = tempfile::tempdir().expect("tempdir");
    let history_dir = dir.path().join("History");
    let core = core(&history_dir).await;

    let owed = backlogged(&core).await;
    core.start_meeting(None, None).await.expect("start");
    assert!(
        core.is_recording().await,
        "the recording is what stands it down"
    );

    let queued = worker_until(&core, std::time::Duration::from_millis(600), |q| {
        q.is_empty()
    })
    .await;
    assert_eq!(
        queued,
        [owed],
        "the backlog is still owed while a Meeting records"
    );
}

#[tokio::test]
async fn with_nothing_recording_the_same_backlog_is_worked() {
    // The control. Without it the test above passes on a worker that never
    // runs anything at all.
    let dir = tempfile::tempdir().expect("tempdir");
    let history_dir = dir.path().join("History");
    let core = core(&history_dir).await;

    backlogged(&core).await;
    assert!(!core.is_recording().await);

    let queued = worker_until(&core, std::time::Duration::from_secs(5), |q| q.is_empty()).await;
    assert!(
        queued.is_empty(),
        "nothing is recording, so the backlog is worked: {queued:?}"
    );
}

#[tokio::test]
async fn a_recording_does_not_stand_down_work_somebody_is_waiting_for() {
    // Auto-Record opening the next call is not a reason to make the person who
    // just finished the last one wait for it. Only bulk work yields.
    let dir = tempfile::tempdir().expect("tempdir");
    let history_dir = dir.path().join("History");
    let core = core(&history_dir).await;

    // `stop_meeting` queues at Front by itself — the real path.
    let just_ended = finished_meeting(&core).await;
    core.start_meeting(None, None)
        .await
        .expect("start the next");
    assert!(core.is_recording().await);

    let queued = worker_until(&core, std::time::Duration::from_secs(5), |q| q.is_empty()).await;
    assert!(
        queued.is_empty(),
        "{just_ended} kept its turn through the next recording: {queued:?}"
    );
}

#[tokio::test]
async fn the_backlog_resumes_when_the_recording_ends_and_the_just_ended_meeting_goes_first() {
    // The whole cycle: stood down, still owed, and picked up again — with the
    // Meeting that just ended ahead of the backlog it was standing aside for.
    let dir = tempfile::tempdir().expect("tempdir");
    let history_dir = dir.path().join("History");
    let core = core(&history_dir).await;

    let owed = backlogged(&core).await;
    core.start_meeting(None, None).await.expect("start");
    let stood_down = worker_until(&core, std::time::Duration::from_millis(600), |q| {
        q.is_empty()
    })
    .await;
    assert_eq!(stood_down, std::slice::from_ref(&owed), "stood down first");

    let recorded = core.stop_meeting().await.expect("stop");
    assert!(!core.is_recording().await);
    assert_eq!(
        core.diarize_status().await.queued,
        [recorded.id.clone(), owed.clone()],
        "the Meeting that just ended is ahead of the backlog that waited for it"
    );

    let queued = worker_until(&core, std::time::Duration::from_secs(5), |q| q.is_empty()).await;
    assert!(
        queued.is_empty(),
        "and once nothing is recording both are worked: {queued:?}"
    );
}

#[tokio::test]
async fn a_stood_down_backlog_survives_a_restart() {
    // It is owed in the record, not in the worker, so quitting during a call
    // does not lose what the re-run has left to do.
    let dir = tempfile::tempdir().expect("tempdir");
    let history_dir = dir.path().join("History");

    let owed = {
        let core = core(&history_dir).await;
        let owed = backlogged(&core).await;
        core.start_meeting(None, None).await.expect("start");
        let queued = worker_until(&core, std::time::Duration::from_millis(600), |q| {
            q.is_empty()
        })
        .await;
        assert_eq!(queued, std::slice::from_ref(&owed));
        owed
    };

    let restarted = core(&history_dir).await;
    assert!(
        restarted.diarization_holds(&owed).await.expect("holds"),
        "the stood-down Meeting is still owed after a restart"
    );
}
