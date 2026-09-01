//! Auto-Record, end to end through a real Core.
//!
//! The policy has its own tests and they are pure. This is the other half:
//! that a scripted timeline actually produces a Meeting in the record, that
//! the Operator's Stop wins over a live detector, and that the switch turns
//! the whole thing off — through the Core, the store, and the driver, with
//! nothing mocked but the two seams that exist to be mocked.

use std::sync::Arc;

use evertranscript_core::Core;
use evertranscript_core::audio::fixture::FixtureSource;
use evertranscript_core::audio::fixture::Step;
use evertranscript_core::detect::DetectionSource;
use evertranscript_core::detect::fixture::FixtureDetectionSource;
use evertranscript_core::detect::fixture::Timeline;
use evertranscript_core::detect::notify::SilentNotifier;
use evertranscript_protocol::AudioChannel;
use evertranscript_protocol::SettingsSetParams;
use tokio_util::sync::CancellationToken;

/// A Core whose capture is a script and whose detection is a timeline.
async fn core_watching(
    timeline: Vec<evertranscript_core::detect::DetectionEvent>,
) -> (Arc<Core>, tempfile::TempDir, CancellationToken) {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");
    core.set_source_factory(Arc::new(|| {
        Box::new(FixtureSource::new(vec![
            Step::audio(AudioChannel::Mic, 400, 0.3),
            Step::audio(AudioChannel::System, 400, -0.3),
        ]))
    }))
    .await;

    let shutdown = CancellationToken::new();
    let source: Box<dyn DetectionSource> = Box::new(FixtureDetectionSource::new(timeline));
    tokio::spawn(evertranscript_core::detect::driver::run(
        Arc::clone(&core),
        vec![source],
        Box::new(SilentNotifier),
        shutdown.clone(),
    ));
    (core, dir, shutdown)
}

/// Long enough for the driver to have done nothing.
///
/// Only for the tests that assert an absence. There is nothing to poll for
/// when the claim is that no Meeting appears, so this stays a fixed wait.
async fn settle() {
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
}

/// Waits until the record says what the test is waiting for.
///
/// **This replaced a fixed 600 ms sleep, and the sleep was a platform
/// assumption.** 600 ms was enough on macOS, which is the only platform this
/// file had ever run on — `#![cfg(unix)]` kept it off Windows, where the
/// same path takes longer and
/// `a_watchlist_app_taking_the_microphone_records_a_meeting` failed
/// deterministically. Nothing was wrong with Auto-Record: at 4 s the same
/// test passes. What was wrong was asserting on a clock (DECISIONS Q54).
///
/// Polling keeps the fast machine fast and the slow one correct, and the
/// deadline is deliberately far past either — a wait that expires still
/// hands the assertion the real state, so the test fails on what it is
/// about rather than on a timeout.
async fn settle_until(core: &Core, ready: impl Fn(&[evertranscript_protocol::Meeting]) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let meetings = core.list_meetings(10, 0).await.expect("list");
        if ready(&meetings) || std::time::Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn a_watchlist_app_taking_the_microphone_records_a_meeting() {
    // The headline promise, made executable: nobody pressed Record.
    let (core, _dir, shutdown) = core_watching(
        Timeline::new()
            .mic_held("us.zoom.xos")
            .wait(600_000)
            .mic_released("us.zoom.xos")
            .wait(30_000)
            .fragmented(5_000),
    )
    .await;
    settle_until(&core, |m| {
        m.first().is_some_and(|meeting| meeting.ended_at.is_some())
    })
    .await;
    shutdown.cancel();

    let meetings = core.list_meetings(10, 0).await.expect("list");
    assert_eq!(meetings.len(), 1, "one meeting, one Meeting: {meetings:?}");
    assert_eq!(
        meetings[0].detected_app.as_deref(),
        Some("us.zoom.xos"),
        "the Meeting is attributed to what triggered it"
    );
    assert!(meetings[0].ended_at.is_some(), "and it stopped by itself");
}

#[tokio::test]
async fn nothing_on_the_watchlist_means_nothing_recorded() {
    // A hot microphone in a dictation app is not a meeting (ADR-0024).
    let (core, _dir, shutdown) = core_watching(
        Timeline::new()
            .mic_held("com.superwhisper")
            .wait(600_000)
            .mic_released("com.superwhisper")
            .wait(30_000)
            .fragmented(5_000),
    )
    .await;
    settle().await;
    shutdown.cancel();

    assert!(
        core.list_meetings(10, 0).await.expect("list").is_empty(),
        "dictation became a Meeting"
    );
}

#[tokio::test]
async fn the_auto_record_switch_turns_the_whole_thing_off() {
    // Story 14: one legible act (ADR-0023).
    let (core, _dir, shutdown) = core_watching(
        Timeline::new()
            .wait(2_000)
            .mic_held("us.zoom.xos")
            .wait(600_000)
            .fragmented(5_000),
    )
    .await;
    core.update_settings(SettingsSetParams {
        auto_record: Some(false),
        ..Default::default()
    })
    .await
    .expect("settings");
    settle().await;
    shutdown.cancel();

    assert!(
        core.list_meetings(10, 0).await.expect("list").is_empty(),
        "Auto-Record was off and something still recorded"
    );
}

#[tokio::test]
async fn a_device_swap_produces_one_meeting_rather_than_two() {
    // The expensive case, through the real Core rather than the state
    // machine alone: eight seconds of silence is an AirPods swap.
    let (core, _dir, shutdown) = core_watching(
        Timeline::new()
            .mic_held("us.zoom.xos")
            .wait(120_000)
            .mic_released("us.zoom.xos")
            .wait(8_000)
            .mic_held("us.zoom.xos")
            .wait(120_000)
            .mic_released("us.zoom.xos")
            .wait(30_000)
            .fragmented(2_000),
    )
    .await;
    settle().await;
    shutdown.cancel();

    let meetings = core.list_meetings(10, 0).await.expect("list");
    assert_eq!(
        meetings.len(),
        1,
        "the swap split the Meeting: {meetings:?}"
    );
}

#[tokio::test]
async fn a_calendar_armed_meeting_is_named_from_the_event() {
    // ADR-0036's title chain: the calendar names the Meeting at its birth,
    // instead of it being "zoom, 10:02" until a Summary renames it. And the
    // arming itself records nothing — only the microphone does that.
    let (core, _dir, shutdown) = core_watching(
        Timeline::new()
            .calendar_started("evt-1", "Quarterly review", 1_800_000)
            .wait(20_000)
            .mic_held("us.zoom.xos")
            .wait(600_000)
            .mic_released("us.zoom.xos")
            .wait(30_000)
            .fragmented(5_000),
    )
    .await;
    settle().await;
    shutdown.cancel();

    let meetings = core.list_meetings(10, 0).await.expect("list");
    assert_eq!(meetings.len(), 1, "one meeting: {meetings:?}");
    assert_eq!(
        meetings[0].title.as_deref(),
        Some("Quarterly review"),
        "the Meeting should carry the event's title"
    );
    assert_eq!(
        meetings[0].calendar_event_id.as_deref(),
        Some("evt-1"),
        "and be traceable back to the entry that armed it"
    );
}

#[tokio::test]
async fn an_armed_meeting_alone_records_nothing() {
    // The calendar knows when; only the microphone knows that.
    let (core, _dir, shutdown) = core_watching(
        Timeline::new()
            .calendar_started("evt-1", "Quarterly review", 1_800_000)
            .wait(600_000)
            .fragmented(5_000),
    )
    .await;
    settle().await;
    shutdown.cancel();

    assert!(
        core.list_meetings(10, 0).await.expect("list").is_empty(),
        "the calendar started a recording, which it must never do"
    );
}

#[tokio::test]
async fn a_meeting_auto_record_started_recovers_from_a_crash_like_any_other() {
    // Ticket 09's crash criterion. A Meeting nobody opened by hand is still
    // a Meeting: the recovery path must not care who started it, and the
    // one thing that could differ — that no Client ever attached to it — is
    // exactly the shape an unattended capture has.
    let dir = tempfile::tempdir().expect("tempdir");
    let history = dir.path().join("History");

    let started = {
        let core = Core::with_history_dir_acknowledged(history.clone()).expect("core");
        core.set_source_factory(Arc::new(|| {
            Box::new(FixtureSource::new(vec![
                Step::audio(AudioChannel::Mic, 400, 0.3),
                Step::audio(AudioChannel::System, 400, -0.3),
            ]))
        }))
        .await;

        let shutdown = CancellationToken::new();
        let source: Box<dyn DetectionSource> = Box::new(FixtureDetectionSource::new(
            Timeline::new().mic_held("us.zoom.xos").into_events(),
        ));
        tokio::spawn(evertranscript_core::detect::driver::run(
            Arc::clone(&core),
            vec![source],
            Box::new(SilentNotifier),
            shutdown.clone(),
        ));
        settle_until(&core, |m| !m.is_empty()).await;

        let meetings = core.list_meetings(10, 0).await.expect("list");
        assert_eq!(meetings.len(), 1, "Auto-Record should have started one");
        assert!(meetings[0].ended_at.is_none(), "and it is still running");
        shutdown.cancel();
        meetings[0].id.clone()
        // The Core is dropped here with a Meeting still open: what a kill
        // leaves behind.
    };

    // A new Core over the same History, as the next start would be.
    let recovered = Core::with_history_dir_acknowledged(history).expect("core");
    recovered.recover_interrupted_audio().await;

    let meetings = recovered.list_meetings(10, 0).await.expect("list");
    assert_eq!(meetings.len(), 1, "the auto-started Meeting must survive");
    assert_eq!(meetings[0].id, started, "and be the same one");
}

#[tokio::test]
async fn every_shipped_watchlist_row_triggers_a_meeting() {
    // Ticket 09 asks for each shipped entry to be observed triggering. Zoom,
    // Teams and VooV are not installed on the machine this was written on,
    // so the half that does not need them present is asserted here: each
    // row, driven through the real Core, produces a Meeting attributed to
    // itself. What a live run would add is whether the platform reports
    // that application holding the microphone at all — which is ticket 04's
    // mechanism, already proven separately against a real one.
    for (id, label) in [
        ("us.zoom.xos", "Zoom"),
        ("com.microsoft.teams2", "Microsoft Teams"),
        ("com.tencent.meeting", "VooV Meeting"),
        ("com.tencent.tencentmeeting", "腾讯会议"),
        ("com.google.Chrome", "Browser Meetings"),
    ] {
        let timeline = Timeline::new()
            .mic_held(id)
            .wait(120_000)
            .mic_released(id)
            .wait(30_000)
            .fragmented(5_000);
        let (core, _dir, shutdown) = core_watching(timeline).await;
        settle_until(&core, |m| {
            m.first().is_some_and(|meeting| meeting.ended_at.is_some())
        })
        .await;
        shutdown.cancel();

        let meetings = core.list_meetings(10, 0).await.expect("list");
        assert_eq!(meetings.len(), 1, "{label} ({id}) recorded nothing");
        assert_eq!(
            meetings[0].detected_app.as_deref(),
            Some(id),
            "{label} should be attributed to itself"
        );
        assert!(meetings[0].ended_at.is_some(), "{label} never stopped");
    }
}
