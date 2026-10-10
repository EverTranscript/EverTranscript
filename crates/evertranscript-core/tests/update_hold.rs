//! An update installs only when nothing would be interrupted, and nothing
//! starts while it does (DECISIONS Q330).
//!
//! The Client asks the Core for a hold before it installs. On Windows the
//! installer stops the Core outright, so a recording that started between
//! "the Core is idle" and the install would be killed; the hold is what
//! closes that gap, which makes it the Core's to enforce rather than the
//! Client's to check.

use std::path::Path;
use std::sync::Arc;

use evertranscript_core::audio::fixture::FixtureSource;
use evertranscript_core::audio::fixture::Step;
use evertranscript_core::server::Core;
use evertranscript_core::store::diarize_queue::Priority;
use evertranscript_protocol::AudioChannel;
use evertranscript_protocol::UpdateBusy;

/// A Core whose capture is scripted: a real device here is what crashed the
/// Windows runner in Q55.
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

/// A finished Meeting with no audio and out of the line: what a worker that
/// is let through drops at once, and what leaves the Core idle.
async fn finished_meeting(core: &Arc<Core>) -> String {
    let meeting = core.start_meeting(None, None).await.expect("start");
    core.stop_meeting().await.expect("stop");
    let (stored, _) = core
        .get_meeting(&meeting.id)
        .await
        .expect("get")
        .expect("meeting");
    if let Some(path) = stored.audio_path {
        std::fs::remove_file(core.history_dir().join(path)).expect("delete the audio");
    }
    core.diarize_cancel(&meeting.id)
        .await
        .expect("out of the line");
    meeting.id
}

#[tokio::test]
async fn a_recording_refuses_the_hold() {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core(&dir.path().join("History")).await;

    core.start_meeting(None, None).await.expect("start");
    let hold = core.hold_for_update().await.expect("hold");
    assert!(!hold.held);
    assert_eq!(hold.busy, Some(UpdateBusy::Recording));
    core.stop_meeting().await.expect("stop");
}

#[tokio::test]
async fn a_meeting_waiting_to_be_diarized_refuses_the_hold() {
    // Idle between two jobs is not idle: the worker starts the next one the
    // moment it looks.
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core(&dir.path().join("History")).await;

    let id = finished_meeting(&core).await;
    core.enqueue_diarization(&id, Priority::Back)
        .await
        .expect("queue");
    let hold = core.hold_for_update().await.expect("hold");
    assert!(!hold.held);
    assert_eq!(hold.busy, Some(UpdateBusy::Diarizing));
}

#[tokio::test]
async fn a_hold_refuses_every_recording_until_it_is_released() {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core(&dir.path().join("History")).await;

    let hold = core.hold_for_update().await.expect("hold");
    assert!(hold.held, "an idle Core grants the hold: {hold:?}");
    let refused = core
        .start_meeting(None, Some("Zoom".into()))
        .await
        .expect_err("nothing records while an update installs");
    assert!(
        refused.to_string().contains("update"),
        "says why: {refused}"
    );
    assert!(!core.is_recording().await);

    // An install that failed hands the Core back at once.
    assert!(!core.release_update_hold().await.held);
    core.start_meeting(None, None).await.expect("records again");
    core.stop_meeting().await.expect("stop");
}

#[tokio::test(start_paused = true)]
async fn a_hold_nobody_releases_lapses_on_its_own() {
    // A Client that crashed holding it must not leave the Core unable to
    // record for the rest of the day.
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core(&dir.path().join("History")).await;

    assert!(core.hold_for_update().await.expect("hold").held);
    core.start_meeting(None, None).await.expect_err("held");
    tokio::time::advance(std::time::Duration::from_secs(3 * 60)).await;
    core.start_meeting(None, None)
        .await
        .expect("past the two-minute lease, recording is allowed again");
    core.stop_meeting().await.expect("stop");
}

#[tokio::test]
async fn work_queued_during_a_hold_waits_for_it() {
    // A hold is granted only on an empty line, so anything in the line now
    // joined after it — a model change's re-run, say — and must not start
    // just as the installer stops the Core.
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core(&dir.path().join("History")).await;
    let id = finished_meeting(&core).await;
    assert!(core.hold_for_update().await.expect("hold").held);
    core.enqueue_diarization(&id, Priority::Front)
        .await
        .expect("queue");

    let shutdown = tokio_util::sync::CancellationToken::new();
    let worker = tokio::spawn(Arc::clone(&core).run_diarization_queue(shutdown.clone()));
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    assert_eq!(
        core.diarize_status().await.expect("status").queued,
        std::slice::from_ref(&id),
        "held: the worker left it in the line"
    );

    core.release_update_hold().await;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !core
        .diarize_status()
        .await
        .expect("status")
        .queued
        .is_empty()
        && std::time::Instant::now() < deadline
    {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        core.diarize_status()
            .await
            .expect("status")
            .queued
            .is_empty(),
        "released: worked at once"
    );
    shutdown.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(5), worker)
        .await
        .expect("the worker did not stop when shutdown was cancelled")
        .expect("the worker panicked");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_recording_and_a_hold_never_both_win() {
    // The race Q329 left open: asked at the same moment, either the
    // recording starts and the hold is refused, or the hold is granted and
    // the recording refused — never a recording the installer then kills.
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core(&dir.path().join("History")).await;

    for _ in 0..25 {
        let (recording, hold) = tokio::join!(
            tokio::spawn({
                let core = Arc::clone(&core);
                async move { core.start_meeting(None, None).await.is_ok() }
            }),
            tokio::spawn({
                let core = Arc::clone(&core);
                async move { core.hold_for_update().await.expect("hold").held }
            }),
        );
        let (recording, held) = (recording.expect("join"), hold.expect("join"));
        assert!(
            !(recording && held),
            "a recording started under a granted hold"
        );
        assert!(recording || held, "one of the two must win");
        if recording {
            let stopped = core.stop_meeting().await.expect("stop");
            core.diarize_cancel(&stopped.id)
                .await
                .expect("out of the line");
        } else {
            core.release_update_hold().await;
        }
    }
}
