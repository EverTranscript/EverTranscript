//! An update installs only when nothing would be interrupted, and nothing
//! starts while it does (DECISIONS Q330, Q333).
//!
//! The Client asks the Core for a hold before it installs. On Windows the
//! installer stops the Core outright, so a recording that started between
//! "the Core is idle" and the install would be killed; the hold is what
//! closes that gap, which makes it the Core's to enforce rather than the
//! Client's to check. It lasts while the asking connection is open and then
//! while the installer runs — never on a timer, because nothing bounds when
//! the installer gets to the Core.

mod common;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use evertranscript_core::Server;
use evertranscript_core::audio::fixture::FixtureSource;
use evertranscript_core::audio::fixture::Step;
use evertranscript_core::client::CoreClient;
use evertranscript_core::server::Core;
use evertranscript_core::store::diarize_queue::Priority;
use evertranscript_protocol::AudioChannel;
use evertranscript_protocol::UpdateBusy;
use evertranscript_protocol::UpdateHoldResponse;
use serde_json::json;

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
/// The connection a test's Client asks on.
const CLIENT: u64 = 7;

/// The installer the Windows Client names, wherever it downloaded it.
const SETUP: &str =
    r"C:\Users\o\AppData\Local\evertranscript-updater\pending\EverTranscript-Setup-1.2.0.exe";

/// Stands in for the installer process, which a test starts and stops by
/// hand; returns the switch.
async fn scripted_installer(core: &Arc<Core>) -> Arc<AtomicBool> {
    let running = Arc::new(AtomicBool::new(false));
    let probe = Arc::clone(&running);
    core.set_installer_probe(Arc::new(move |path| {
        path == Path::new(SETUP) && probe.load(Ordering::SeqCst)
    }))
    .await;
    running
}

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
    let hold = core
        .hold_for_update(CLIENT, Vec::new())
        .await
        .expect("hold");
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
    let hold = core
        .hold_for_update(CLIENT, Vec::new())
        .await
        .expect("hold");
    assert!(!hold.held);
    assert_eq!(hold.busy, Some(UpdateBusy::Diarizing));
}

#[tokio::test]
async fn a_hold_refuses_every_recording_until_its_connection_closes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core(&dir.path().join("History")).await;

    let hold = core
        .hold_for_update(CLIENT, Vec::new())
        .await
        .expect("hold");
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

    // An install that failed, or a Client that crashed, closes the
    // connection, and with no installer named that hands the Core back.
    core.connection_closed(CLIENT).await;
    core.start_meeting(None, None).await.expect("records again");
    core.stop_meeting().await.expect("stop");
}

#[tokio::test(start_paused = true)]
async fn an_installer_that_starts_late_still_holds_the_core() {
    // The race Q332 found in Q330's two-minute lease: the Client quits, and
    // the installer it started gets to stopping the Core whenever it does.
    // Ten minutes on, the hold still stands for as long as the installer
    // runs, and ends the moment it exits.
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core(&dir.path().join("History")).await;
    let installer = scripted_installer(&core).await;

    let hold = core
        .hold_for_update(CLIENT, vec![SETUP.into()])
        .await
        .expect("hold");
    assert!(hold.held);
    installer.store(true, Ordering::SeqCst);
    core.connection_closed(CLIENT).await;

    tokio::time::advance(std::time::Duration::from_secs(10 * 60)).await;
    core.start_meeting(None, Some("Teams".into()))
        .await
        .expect_err("the installer that will stop this Core is still running");
    assert!(!core.is_recording().await);

    installer.store(false, Ordering::SeqCst);
    core.start_meeting(None, None)
        .await
        .expect("the installer is gone, so nothing can stop a recording now");
    core.stop_meeting().await.expect("stop");
}

#[tokio::test]
async fn an_install_that_never_ran_hands_the_core_back() {
    // quitAndInstall failed, or the installer was cancelled before it got
    // anywhere: the Client is gone and nothing runs from the installer.
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core(&dir.path().join("History")).await;
    let _installer = scripted_installer(&core).await;

    assert!(
        core.hold_for_update(CLIENT, vec![SETUP.into()])
            .await
            .expect("hold")
            .held
    );
    core.connection_closed(CLIENT).await;
    core.start_meeting(None, None)
        .await
        .expect("no installer runs, so the hold is over");
    core.stop_meeting().await.expect("stop");
}

#[tokio::test]
async fn a_restart_needs_the_hold_and_starts_nothing_after() {
    // The catch-up (Q12): a new Client stops an older Core so its own can
    // start. Asked without the hold, it is refused — the hold is what keeps
    // a recording from starting between the Client's look and the stop.
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core(&dir.path().join("History")).await;
    let shutdown = tokio_util::sync::CancellationToken::new();
    core.set_shutdown(shutdown.clone());

    core.restart_for_update(CLIENT)
        .await
        .expect_err("no hold, no restart");
    assert!(!shutdown.is_cancelled());

    assert!(
        core.hold_for_update(CLIENT, Vec::new())
            .await
            .expect("hold")
            .held
    );
    core.restart_for_update(CLIENT).await.expect("restart");
    assert!(shutdown.is_cancelled(), "the daemon is told to stop");

    // The Client's connection closes as the Core goes down; nothing starts.
    core.connection_closed(CLIENT).await;
    core.start_meeting(None, Some("Zoom".into()))
        .await
        .expect_err("a Core stopping for an update records nothing");
}

#[tokio::test]
async fn a_hold_on_another_connection_does_not_allow_a_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core(&dir.path().join("History")).await;
    let shutdown = tokio_util::sync::CancellationToken::new();
    core.set_shutdown(shutdown.clone());

    assert!(
        core.hold_for_update(CLIENT, Vec::new())
            .await
            .expect("hold")
            .held
    );
    core.restart_for_update(CLIENT + 1)
        .await
        .expect_err("only the holder restarts");
    assert!(!shutdown.is_cancelled());
}

#[tokio::test]
async fn work_queued_during_a_hold_waits_for_it() {
    // A hold is granted only on an empty line, so anything in the line now
    // joined after it — a model change's re-run, say — and must not start
    // just as the installer stops the Core.
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core(&dir.path().join("History")).await;
    let id = finished_meeting(&core).await;
    assert!(
        core.hold_for_update(CLIENT, Vec::new())
            .await
            .expect("hold")
            .held
    );
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

    core.connection_closed(CLIENT).await;
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
                async move {
                    core.hold_for_update(CLIENT, Vec::new())
                        .await
                        .expect("hold")
                        .held
                }
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
            core.connection_closed(CLIENT).await;
        }
    }
}

#[tokio::test]
async fn over_a_real_connection_the_hold_ends_when_the_client_goes() {
    // What a Client that crashed, or quit with no installer to run, does:
    // its socket closes, and the server is what turns that into the end of
    // the hold.
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core(&dir.path().join("History")).await;
    let endpoint = common::endpoint(dir.path());
    let listener = evertranscript_core::transport::bind(&endpoint)
        .await
        .expect("bind");
    let (events_tx, events_rx) = tokio::sync::mpsc::channel(64);
    let shutdown = tokio_util::sync::CancellationToken::new();
    tokio::spawn(Server::new(Arc::clone(&core)).run(events_rx, shutdown.clone()));
    tokio::spawn(evertranscript_core::transport::serve(
        listener,
        events_tx,
        shutdown.clone(),
    ));

    let mut client = CoreClient::connect_to(&endpoint).await.expect("connect");
    client
        .initialize("test-client", "0.0.0")
        .await
        .expect("initialize");
    let hold: UpdateHoldResponse = client
        .request("update/hold", Some(json!({})))
        .await
        .expect("hold");
    assert!(hold.held);
    core.start_meeting(None, None)
        .await
        .expect_err("held while the Client is connected");

    drop(client);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let started = loop {
        match core.start_meeting(None, None).await {
            Ok(meeting) => break meeting,
            Err(_) if std::time::Instant::now() < deadline => {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            Err(error) => panic!("the hold outlived its connection: {error}"),
        }
    };
    assert_eq!(core.stop_meeting().await.expect("stop").id, started.id);
    shutdown.cancel();
}
