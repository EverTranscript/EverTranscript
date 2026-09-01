//! Nothing is captured before the Operator acknowledges the Briefing.
//!
//! ADR-0023 is explicit that "Auto-Record is on by default" means the toggle
//! ships On — *not* that recording precedes consent education. That makes
//! this a pre-capture invariant rather than a UI convention, so it is
//! enforced in the Core where no Client can route around it, and tested
//! here.

use std::sync::Arc;

use evertranscript_core::Core;
use evertranscript_core::audio::fixture::FixtureSource;
use evertranscript_core::settings::Settings;
use evertranscript_protocol::SettingsSetParams;

async fn fresh_install() -> (Arc<Core>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = Core::with_paths(dir.path().join("History"), dir.path().join("settings.json"))
        .expect("core");
    core.set_source_factory(Arc::new(|| Box::new(FixtureSource::simple(200))))
        .await;
    (core, dir)
}

#[tokio::test]
async fn a_fresh_install_refuses_to_record() {
    let (core, _dir) = fresh_install().await;

    let error = core
        .start_meeting(None, Some("Zoom".into()))
        .await
        .expect_err("recording before acknowledgment must be refused");
    assert!(
        error.to_string().contains("briefing"),
        "the refusal should say why and how to fix it: {error}"
    );

    // And nothing was created: not a Meeting row, not a Mirror, not a file.
    let meetings = core.list_meetings(10, 0).await.expect("list");
    assert!(
        meetings.is_empty(),
        "a refused recording must leave no trace"
    );
}

#[tokio::test]
async fn acknowledging_permits_recording() {
    let (core, _dir) = fresh_install().await;

    core.update_settings(SettingsSetParams {
        briefing_acknowledged: Some(true),
        ..Default::default()
    })
    .await
    .expect("acknowledge");

    let meeting = core
        .start_meeting(None, Some("Zoom".into()))
        .await
        .expect("recording is permitted once acknowledged");
    core.stop_meeting().await.expect("stop");
    assert!(!meeting.id.is_empty());
}

#[tokio::test]
async fn acknowledgment_survives_a_restart() {
    // It is stored per installation, so the Operator is asked once.
    let dir = tempfile::tempdir().expect("tempdir");
    let history = dir.path().join("History");
    let settings = dir.path().join("settings.json");

    {
        let core = Core::with_paths(history.clone(), settings.clone()).expect("core");
        core.update_settings(SettingsSetParams {
            briefing_acknowledged: Some(true),
            ..Default::default()
        })
        .await
        .expect("acknowledge");
    }

    let restarted = Core::with_paths(history, settings).expect("core");
    assert!(
        restarted.briefing_acknowledged().await,
        "the Operator must not be asked again on every launch"
    );
}

#[tokio::test]
async fn a_client_cannot_un_acknowledge() {
    // Consent that a Client can withdraw programmatically is a toggle, not
    // an invariant. Withdrawal is deleting the settings file or the app.
    let (core, _dir) = fresh_install().await;

    core.update_settings(SettingsSetParams {
        briefing_acknowledged: Some(true),
        ..Default::default()
    })
    .await
    .expect("acknowledge");

    let settings = core
        .update_settings(SettingsSetParams {
            briefing_acknowledged: Some(false),
            ..Default::default()
        })
        .await
        .expect("the request itself is not an error");

    assert!(settings.briefing_acknowledged, "acknowledgment is one-way");
}

#[tokio::test]
async fn the_settings_that_ship_on_are_on_and_the_one_that_matters_is_not() {
    let settings = Settings::default();
    assert!(
        !settings.briefing_acknowledged,
        "consent is never a default"
    );
    assert!(settings.auto_record, "Auto-Record ships On (ADR-0023)");
    assert!(
        settings.launch_at_login,
        "the Core is the login item (ADR-0026)"
    );
}

#[tokio::test]
async fn changing_one_setting_leaves_the_others_alone() {
    let (core, _dir) = fresh_install().await;
    core.update_settings(SettingsSetParams {
        briefing_acknowledged: Some(true),
        ..Default::default()
    })
    .await
    .expect("acknowledge");

    let settings = core
        .update_settings(SettingsSetParams {
            auto_record: Some(false),
            ..Default::default()
        })
        .await
        .expect("update");

    assert!(!settings.auto_record, "the field asked for changed");
    assert!(
        settings.briefing_acknowledged,
        "and the ones not asked for did not"
    );
}
