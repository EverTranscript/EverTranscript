//! The Title Chain's third slot, which had never fired.
//!
//! ADR-0030 as amended by ADR-0036 ratifies *manual > calendar > Suggested
//! Title > detected-app placeholder*, and the transcript-derived slot was
//! plumbed and unreachable: the heading extractor existed, was tested, and
//! nothing in production called it. A Meeting whose Summary opened with a
//! perfectly good name stayed "Zoom, 2026-09-01" forever.
//!
//! These drive the whole path a Client drives — record, summarize, read the
//! Meeting back — with a scripted Backend, because what is being tested is
//! everything around generation rather than generation itself. No model.

#![cfg(unix)]

use std::sync::Arc;

use anyhow::Result;
use evertranscript_core::Core;
use evertranscript_core::asr::Transcriber;
use evertranscript_core::asr::Transcript;
use evertranscript_core::audio::fixture::FixtureSource;
use evertranscript_core::audio::fixture::Step;
use evertranscript_core::summary::fake::FakeBackend;
use evertranscript_protocol::AudioChannel;
use evertranscript_protocol::SettingsSetParams;

/// Says one line, so a Meeting has a transcript to summarize.
struct SaysSomething;

impl Transcriber for SaysSomething {
    fn transcribe(&mut self, _samples: &[f32], _previous: Option<&str>) -> Result<Transcript> {
        Ok(Transcript {
            text: "We agreed to defer the hiring plan until October.".to_string(),
            confidence: 0.9,
            decode_time: std::time::Duration::from_millis(1),
            language: Some("en".to_string()),
        })
    }

    fn describe(&self) -> String {
        "says something".to_string()
    }
}

fn speech() -> Vec<Step> {
    vec![
        Step::audio(AudioChannel::Mic, 4_000, 0.3),
        Step::audio(AudioChannel::Mic, 1_500, 0.0),
    ]
}

/// A Core with a recorded Meeting in it, and a Backend that will answer with
/// `summary` when asked.
async fn core_with_a_recorded_meeting(
    dir: &std::path::Path,
    summary: &'static str,
) -> (Arc<Core>, String) {
    let core = Core::with_history_dir_acknowledged(dir.join("History")).expect("core");
    core.set_source_factory(Arc::new(|| Box::new(FixtureSource::new(speech()))))
        .await;
    core.set_transcriber_factory(Arc::new(|| {
        Some(Box::new(SaysSomething) as Box<dyn Transcriber>)
    }))
    .await;
    core.set_summary_backend_factory(Arc::new(move || {
        (Box::new(FakeBackend::returning(summary)), None)
    }));
    // A Backend must be chosen outright — the Knob has no default (ADR-0013).
    core.update_settings(SettingsSetParams {
        summary_backend: Some("local".to_string()),
        ..Default::default()
    })
    .await
    .expect("settings");

    let meeting = core
        .start_meeting(None, Some("Zoom".to_string()))
        .await
        .expect("start");
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    core.stop_meeting().await.expect("stop");
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    (core, meeting.id)
}

async fn title_of(core: &Core, id: &str) -> Option<String> {
    core.get_meeting(id)
        .await
        .expect("get")
        .expect("the Meeting")
        .0
        .title
}

#[tokio::test]
async fn a_summary_names_an_untitled_meeting() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (core, id) = core_with_a_recorded_meeting(
        dir.path(),
        "# Hiring plan deferred\n\nThe team agreed to wait until October.",
    )
    .await;

    assert_eq!(title_of(&core, &id).await, None, "starts unnamed");
    core.summarize_meeting(&id).await.expect("summarize");

    assert_eq!(
        title_of(&core, &id).await.as_deref(),
        Some("Hiring plan deferred"),
        "the Summary's first heading should have named the Meeting"
    );
}

#[tokio::test]
async fn the_mirror_is_renamed_when_a_summary_names_a_meeting() {
    // The name has to reach the file on disk, not just the row: the Mirror is
    // what an Operator sees in Finder, and a record that disagrees with itself
    // about its own name is the bug this chain exists to avoid.
    let dir = tempfile::tempdir().expect("tempdir");
    let (core, id) =
        core_with_a_recorded_meeting(dir.path(), "# Budget review\n\nDeferred to Friday.").await;

    core.summarize_meeting(&id).await.expect("summarize");
    let meeting = core
        .get_meeting(&id)
        .await
        .expect("get")
        .expect("the Meeting")
        .0;

    let filename = meeting
        .mirror_filename
        .expect("the Mirror should have a name");
    assert!(
        filename.contains("budget-review"),
        "the Mirror should carry the suggested name, got {filename}"
    );
}

#[tokio::test]
async fn a_manual_name_outranks_the_summarys_suggestion() {
    // Slot one beats slot three. The machine never overwrites a human's word.
    let dir = tempfile::tempdir().expect("tempdir");
    let (core, id) =
        core_with_a_recorded_meeting(dir.path(), "# A name the model chose\n\nBody.").await;

    core.retitle_meeting(&id, "What Frank called it")
        .await
        .expect("retitle");
    core.summarize_meeting(&id).await.expect("summarize");

    assert_eq!(
        title_of(&core, &id).await.as_deref(),
        Some("What Frank called it"),
        "a manual name must survive summarizing"
    );
}

#[tokio::test]
async fn regenerating_a_summary_never_refreshes_the_name() {
    // Write-once. The Suggested Title is a name like any other the moment it
    // lands, so a second Summary — even a better one — leaves it alone.
    let dir = tempfile::tempdir().expect("tempdir");
    let (core, id) = core_with_a_recorded_meeting(dir.path(), "# First name\n\nBody.").await;

    core.summarize_meeting(&id).await.expect("first summary");
    assert_eq!(title_of(&core, &id).await.as_deref(), Some("First name"));

    core.set_summary_backend_factory(Arc::new(|| {
        (
            Box::new(FakeBackend::returning("# Second name\n\nBetter body.")),
            None,
        )
    }));
    core.summarize_meeting(&id).await.expect("second summary");

    assert_eq!(
        title_of(&core, &id).await.as_deref(),
        Some("First name"),
        "a regenerated Summary must not rename the Meeting"
    );
}

#[tokio::test]
async fn clearing_the_name_re_opens_the_slot() {
    // The escape hatch ticket 01 built, seen from this side: clearing a name
    // means the next Summary may name it again. Without the empty-string
    // normalisation this would silently never fire.
    let dir = tempfile::tempdir().expect("tempdir");
    let (core, id) = core_with_a_recorded_meeting(dir.path(), "# First name\n\nBody.").await;

    core.summarize_meeting(&id).await.expect("summarize");
    assert_eq!(title_of(&core, &id).await.as_deref(), Some("First name"));

    core.retitle_meeting(&id, "   ").await.expect("clear");
    assert_eq!(
        title_of(&core, &id).await,
        None,
        "clearing leaves it unnamed"
    );

    core.set_summary_backend_factory(Arc::new(|| {
        (
            Box::new(FakeBackend::returning("# A fresh name\n\nBody.")),
            None,
        )
    }));
    core.summarize_meeting(&id).await.expect("re-summarize");

    assert_eq!(
        title_of(&core, &id).await.as_deref(),
        Some("A fresh name"),
        "a cleared name should let the next Summary name the Meeting"
    );
}

#[tokio::test]
async fn a_headingless_summary_proposes_nothing() {
    // What the shipped 0.5B usually produces (DECISIONS Q45): no `# ` heading
    // at all. The chain falls through to the placeholder rather than
    // inventing a name out of the first line of prose.
    let dir = tempfile::tempdir().expect("tempdir");
    let (core, id) =
        core_with_a_recorded_meeting(dir.path(), "None noted.\n\nAction items:\n\n- None noted.")
            .await;

    core.summarize_meeting(&id).await.expect("summarize");

    assert_eq!(
        title_of(&core, &id).await,
        None,
        "a Summary with no heading must not name the Meeting"
    );
}

#[tokio::test]
async fn a_summary_that_was_never_stored_names_nothing() {
    // Cancellation and failure store neither Summary nor name. A Meeting
    // named by a Summary that does not exist would be the worst of both.
    let dir = tempfile::tempdir().expect("tempdir");
    let (core, id) = core_with_a_recorded_meeting(dir.path(), "# Never stored\n\nBody.").await;

    core.set_summary_backend_factory(Arc::new(|| {
        (
            Box::new(FakeBackend::failing(
                evertranscript_core::summary::fake::Failure::Unavailable,
            )),
            None,
        )
    }));
    let result = core.summarize_meeting(&id).await;
    assert!(result.is_err(), "an unavailable Backend should not succeed");

    assert_eq!(
        title_of(&core, &id).await,
        None,
        "a failed Summary must leave the Meeting unnamed"
    );
}
