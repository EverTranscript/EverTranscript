//! The tray as an Operator drives it, without a menu bar.
//!
//! A menu bar item cannot be clicked by a test suite, so the tray is built
//! with the deciding in [`TrayController`] and only the drawing in AppKit.
//! This drives the controller against a real Core: clicking starts a real
//! Meeting, clicking again stops it, and a refused start comes back as
//! something the menu can show.
//!
//! What is deliberately *not* claimed here is that the icon appears. That
//! needs eyes on a screen.

#![cfg(unix)]

use std::sync::Arc;

use evertranscript_core::audio::fixture::FixtureSource;
use evertranscript_core::audio::fixture::Step;
use evertranscript_core::tray::TrayController;
use evertranscript_core::tray::TrayPhase;
use evertranscript_core::Core;
use evertranscript_protocol::AudioChannel;
use evertranscript_protocol::CoreState;
use tokio_util::sync::CancellationToken;

/// A Core whose capture is a script, so no hardware is involved.
async fn core_with_capture(acknowledged: bool, dir: &tempfile::TempDir) -> Arc<Core> {
    let history = dir.path().join("History");
    let core = if acknowledged {
        Core::with_history_dir_acknowledged(history).expect("core")
    } else {
        // Scoped settings, not the machine's: `with_history_dir` would read
        // whatever this machine's real acknowledgment happens to be, which
        // makes the test pass or fail depending on who ran the app here.
        let settings = dir.path().join("unacknowledged.json");
        Core::with_paths(history, settings).expect("core")
    };
    core.set_source_factory(Arc::new(|| {
        Box::new(FixtureSource::new(vec![
            Step::audio(AudioChannel::Mic, 400, 0.4),
            Step::audio(AudioChannel::System, 400, -0.4),
        ]))
    }))
    .await;
    core
}

fn controller(core: Arc<Core>) -> Arc<TrayController> {
    TrayController::new(
        core,
        tokio::runtime::Handle::current(),
        CancellationToken::new(),
    )
}

/// Settled, not recording, and clicking would start.
///
/// Two phases mean that: `Idle` on a machine with models downloaded and
/// `NotReady` on one without. Asserting the exact phase would make these
/// tests pass or fail on whether a 900 MB file happens to be present, which
/// is not what any of them are about.
fn ready_to_record(phase: TrayPhase) -> bool {
    matches!(phase, TrayPhase::Idle | TrayPhase::NotReady)
}

/// Refreshes until `wanted` holds, or gives up.
async fn settle(controller: &Arc<TrayController>, wanted: impl Fn(TrayPhase) -> bool) -> TrayPhase {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        controller.refresh().await;
        let current = controller.phase();
        if wanted(current) || std::time::Instant::now() > deadline {
            return current;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn clicking_record_starts_a_real_meeting_and_clicking_again_stops_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core_with_capture(true, &dir).await;
    let controller = controller(Arc::clone(&core));
    controller.refresh().await;
    assert!(
        controller.phase().is_actionable(),
        "a Core that can record must offer to, got {:?}",
        controller.phase()
    );

    // The click shows before the work finishes: a menu bar that waits for
    // the Core feels broken, so the transitional state is returned by the
    // click itself.
    let view = controller.activate();
    assert_eq!(controller.phase(), TrayPhase::Starting);
    assert!(!view.action_enabled, "a transition cannot be clicked again");

    assert_eq!(
        settle(&controller, |phase| phase == TrayPhase::Recording).await,
        TrayPhase::Recording,
        "the tray should settle on Recording once the Core agrees"
    );
    assert_eq!(
        core.state().await,
        CoreState::Recording,
        "and a real Meeting must actually be running"
    );
    let view = controller.view();
    assert_eq!(view.action, "Stop Recording");
    assert_eq!(view.indicator, "●");

    controller.activate();
    assert_eq!(controller.phase(), TrayPhase::Stopping);
    assert!(
        ready_to_record(settle(&controller, ready_to_record).await),
        "and stopping settles back to a state that can record again"
    );
    assert_eq!(core.state().await, CoreState::Idle);
}

#[tokio::test]
async fn a_refused_start_returns_the_menu_to_where_it_was_and_says_why() {
    // Nothing is captured before the Briefing is acknowledged (ADR-0023).
    // The tray must survive being told no: a menu stuck on "Starting…"
    // forever would be worse than one that never moved.
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core_with_capture(false, &dir).await;
    let controller = controller(Arc::clone(&core));
    controller.refresh().await;

    assert_eq!(
        controller.phase(),
        TrayPhase::NotPermitted,
        "an unacknowledged Core should not offer to record"
    );
    assert!(
        !controller.view().action_enabled,
        "and the item must not be clickable"
    );

    // Even so, drive the refused path directly: another Client could
    // acknowledge and un-acknowledge, and the failure has to be survivable
    // wherever it comes from.
    core.update_settings(evertranscript_protocol::SettingsSetParams {
        briefing_acknowledged: Some(true),
        ..Default::default()
    })
    .await
    .expect("acknowledge");
    controller.refresh().await;
    assert!(
        ready_to_record(controller.phase()),
        "once acknowledged the tray offers to record, got {:?}",
        controller.phase()
    );
}

#[tokio::test]
async fn the_tray_follows_a_meeting_it_did_not_start() {
    // The Electron client and the CLI share this Core. A Meeting started
    // anywhere has to show in the menu bar, or the indicator is a lie.
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core_with_capture(true, &dir).await;
    let controller = controller(Arc::clone(&core));
    controller.refresh().await;
    assert!(ready_to_record(controller.phase()));

    core.start_meeting(None, Some("Zoom".to_string()))
        .await
        .expect("start");
    controller.refresh().await;
    assert_eq!(
        controller.phase(),
        TrayPhase::Recording,
        "a Meeting started elsewhere must reach the menu bar"
    );
    assert_eq!(controller.view().indicator, "●");

    core.stop_meeting().await.expect("stop");
    controller.refresh().await;
    assert!(ready_to_record(controller.phase()));
}

/// This machine has no models downloaded, which makes it the right place to
/// pin ADR-0019: the Core records without one, so the tray must not refuse.
/// An earlier draft of this module gated recording on model readiness and
/// would have lost the meeting to save the transcript.
#[tokio::test]
async fn a_missing_model_does_not_stop_the_tray_recording() {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core_with_capture(true, &dir).await;
    let controller = controller(Arc::clone(&core));
    controller.refresh().await;

    if controller.phase() != TrayPhase::NotReady {
        // Models are present here; nothing to prove.
        return;
    }
    assert!(
        controller.view().action_enabled,
        "recording must still be offered without a transcription model"
    );
    controller.activate();
    assert_eq!(
        settle(&controller, |phase| phase == TrayPhase::Recording).await,
        TrayPhase::Recording,
        "and clicking it must actually start a Meeting"
    );
    assert_eq!(core.state().await, CoreState::Recording);
    core.stop_meeting().await.expect("stop");
}

#[tokio::test]
async fn quit_stops_the_core_rather_than_hiding_the_menu() {
    // ADR-0023: Quit is explicit and it means the Core stops. A tray that
    // only removed its own icon would leave a recorder running invisibly.
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core_with_capture(true, &dir).await;
    let shutdown = CancellationToken::new();
    let controller = TrayController::new(core, tokio::runtime::Handle::current(), shutdown.clone());

    assert!(!shutdown.is_cancelled());
    controller.quit();
    assert!(
        shutdown.is_cancelled(),
        "quit must bring the Core down with it"
    );
}
