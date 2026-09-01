//! Map-reduce, in production at last, and the Knob choosing once.
//!
//! The chunking path existed, was tested, and was called by nothing: the
//! server built a single request out of an entire meeting, so a ninety-minute
//! recording — this product's core case — went to the Backend whole. These
//! tests drive the path a Client drives and count what the Backend was asked,
//! which is the only way to tell chunking from a very long single request.
//!
//! The behaviours here were previously asserted against a function nobody
//! called. They now belong to the summarize path, so they are tested where an
//! Operator's record can actually feel them.

#![cfg(unix)]

use std::sync::Arc;

use evertranscript_core::Core;
use evertranscript_core::store::meetings;
use evertranscript_core::summary::BackendIdentity;
use evertranscript_core::summary::fake::Failure;
use evertranscript_core::summary::fake::FakeBackend;
use evertranscript_core::summary::fake::Response;
use evertranscript_protocol::AudioChannel;
use evertranscript_protocol::SettingsSetParams;

/// A Meeting with `lines` of transcript already in the store.
///
/// Written straight to the store rather than recorded through a fixture
/// source: this file is about what happens *after* a transcript exists, and
/// forty thousand characters of speech is not something to synthesise in real
/// time.
async fn meeting_of(core: &Core, lines: usize) -> String {
    let meeting = core
        .start_meeting(None, Some("Zoom".to_string()))
        .await
        .expect("start");
    let id = meeting.id.clone();
    core.store()
        .write(move |connection| {
            for index in 0..lines {
                meetings::append_segment(
                    connection,
                    &id,
                    AudioChannel::Mic,
                    index as i64 * 5_000,
                    index as i64 * 5_000 + 4_000,
                    &format!(
                        "Line {index}: we discussed the quarterly plan and what it means \
                         for the team's commitments over the next few weeks."
                    ),
                )?;
            }
            Ok(())
        })
        .await
        .expect("segments");
    core.stop_meeting().await.expect("stop");
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    meeting.id
}

async fn core_in(dir: &std::path::Path, backend: &'static str) -> Arc<Core> {
    let core = Core::with_history_dir_acknowledged(dir.join("History")).expect("core");
    core.update_settings(SettingsSetParams {
        summary_backend: Some(backend.to_string()),
        // ADR-0013: choosing Cloud requires accepting the one-time warning
        // outright. The gate is real — the first draft of this test hit it.
        summary_cloud_warning_accepted: Some(true),
        ..Default::default()
    })
    .await
    .expect("settings");
    core
}

#[tokio::test]
async fn a_short_meeting_is_still_one_request() {
    // The common case must pay nothing for the long one.
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core_in(dir.path(), "local").await;
    let calls = Arc::new(std::sync::Mutex::new(0usize));
    let counter = Arc::clone(&calls);
    core.set_summary_backend_factory(Arc::new(move || {
        *counter.lock().unwrap() += 1;
        (Box::new(FakeBackend::returning("# Short\n\nBody.")), None)
    }));

    let id = meeting_of(&core, 3).await;
    let markdown = core.summarize_meeting(&id).await.expect("summarize");

    assert!(markdown.contains("Short"));
    assert_eq!(*calls.lock().unwrap(), 1, "one Backend was built");
}

#[tokio::test]
async fn a_long_meeting_is_chunked_rather_than_sent_whole() {
    // The defect this ticket exists for: before it, this was one request no
    // matter how long the meeting.
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core_in(dir.path(), "local").await;
    let backend = FakeBackend::returning("# Part\n\nBody.");
    let prompts = backend.prompts();
    let backend = Arc::new(std::sync::Mutex::new(Some(backend)));
    core.set_summary_backend_factory(Arc::new(move || {
        (
            Box::new(backend.lock().unwrap().take().expect("built once")),
            None,
        )
    }));

    let id = meeting_of(&core, 400).await;
    core.summarize_meeting(&id).await.expect("summarize");

    let seen = prompts.lock().unwrap().len();
    assert!(
        seen > 2,
        "a long meeting should have produced several chunk requests plus a reduce, got {seen}"
    );
}

#[tokio::test]
async fn the_first_chunk_chooses_the_backend_for_the_whole_run() {
    // Choose-once. A cloud Backend that cannot serve the first chunk sends the
    // entire run to local, and the label names local — never a mixture.
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core_in(dir.path(), "openai").await;
    let local = FakeBackend::returning("# Local answered\n\nBody.");
    let local_prompts = local.prompts();
    let parts = Arc::new(std::sync::Mutex::new(Some((
        FakeBackend::cloud("OpenAI", vec![Response::Fails(Failure::Unavailable)]),
        local,
    ))));
    core.set_summary_backend_factory(Arc::new(move || {
        let (cloud, local) = parts.lock().unwrap().take().expect("built once");
        (Box::new(cloud), Some(Box::new(local)))
    }));

    let id = meeting_of(&core, 400).await;
    core.summarize_meeting(&id).await.expect("summarize");

    let meeting = core
        .get_meeting(&id)
        .await
        .expect("get")
        .expect("the Meeting")
        .0;
    let label = meeting.summary_backend.expect("a Backend label");
    assert!(
        label.starts_with("Local"),
        "the whole run fell back, so the label must say local, got {label}"
    );
    assert!(
        local_prompts.lock().unwrap().len() > 2,
        "local should have served every chunk, not just the first"
    );
}

#[tokio::test]
async fn one_failed_chunk_does_not_lose_the_whole_meeting() {
    // Five parts of six is a usable record of the meeting; none is not.
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core_in(dir.path(), "local").await;
    let backend = Arc::new(std::sync::Mutex::new(Some(FakeBackend::scripted(
        BackendIdentity::LocalSidecar {
            model: "fake".into(),
        },
        vec![
            Response::Text("# First\n\nBody.".into()),
            Response::Fails(Failure::TimedOut),
            Response::Text("# Rest\n\nBody.".into()),
        ],
    ))));
    core.set_summary_backend_factory(Arc::new(move || {
        (
            Box::new(backend.lock().unwrap().take().expect("built once")),
            None,
        )
    }));

    let id = meeting_of(&core, 400).await;
    let markdown = core.summarize_meeting(&id).await.expect("summarize");

    assert!(
        !markdown.trim().is_empty(),
        "a chunk failing must not empty the Summary"
    );
}

#[tokio::test]
async fn a_partial_summary_says_so_in_the_record() {
    // Ticket 03 tolerates the loss; this is where the Operator learns of it.
    // The Core's log knows already, and the Operator cannot read the log.
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core_in(dir.path(), "local").await;
    let backend = Arc::new(std::sync::Mutex::new(Some(FakeBackend::scripted(
        BackendIdentity::LocalSidecar {
            model: "fake".into(),
        },
        vec![
            Response::Text("# First\n\nBody.".into()),
            Response::Fails(Failure::TimedOut),
            Response::Text("# Rest\n\nBody.".into()),
        ],
    ))));
    core.set_summary_backend_factory(Arc::new(move || {
        (
            Box::new(backend.lock().unwrap().take().expect("built once")),
            None,
        )
    }));

    let id = meeting_of(&core, 400).await;
    core.summarize_meeting(&id).await.expect("summarize");

    let meeting = core
        .get_meeting(&id)
        .await
        .expect("get")
        .expect("the Meeting")
        .0;
    let gaps = meeting
        .summary_gaps
        .expect("a Summary that lost a chunk must say so");
    assert!(
        gaps.contains("could not be summarized"),
        "the note should say what happened, got {gaps}"
    );

    // And it has to reach the folder the Operator actually reads.
    let mirror = meeting.mirror_filename.expect("a Mirror");
    let body = std::fs::read_to_string(dir.path().join("History").join(&mirror))
        .expect("the Mirror on disk");
    assert!(
        body.contains("This Summary is incomplete"),
        "the Mirror must disclose the gap: {body}"
    );
}

#[tokio::test]
async fn a_complete_summary_says_nothing() {
    // The other half, and the one that keeps the notice meaningful: a run
    // that lost nothing must not carry a disclaimer.
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core_in(dir.path(), "local").await;
    core.set_summary_backend_factory(Arc::new(|| {
        (
            Box::new(FakeBackend::returning("# All of it\n\nBody.")),
            None,
        )
    }));

    let id = meeting_of(&core, 400).await;
    core.summarize_meeting(&id).await.expect("summarize");

    let meeting = core
        .get_meeting(&id)
        .await
        .expect("get")
        .expect("the Meeting")
        .0;
    assert_eq!(
        meeting.summary_gaps, None,
        "a complete Summary must not claim to be partial"
    );
}

#[tokio::test]
async fn a_failed_reduce_keeps_the_parts_rather_than_wasting_every_call_before_it() {
    // The parts are still a record of the meeting. Discarding them because the
    // last call timed out would waste every call before it.
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core_in(dir.path(), "local").await;
    let backend = Arc::new(std::sync::Mutex::new(Some(FakeBackend::scripted(
        BackendIdentity::LocalSidecar {
            model: "fake".into(),
        },
        vec![
            Response::Text("# Part one\n\nAn early decision.".into()),
            Response::Text("# Part two\n\nA later decision.".into()),
            Response::Fails(Failure::TimedOut),
        ],
    ))));
    core.set_summary_backend_factory(Arc::new(move || {
        (
            Box::new(backend.lock().unwrap().take().expect("built once")),
            None,
        )
    }));

    let id = meeting_of(&core, 400).await;
    let markdown = core.summarize_meeting(&id).await.expect("summarize");

    assert!(
        markdown.contains("An early decision"),
        "the surviving parts must reach the record: {markdown}"
    );
}

#[tokio::test]
async fn every_chunk_failing_is_an_error_rather_than_an_empty_summary() {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core_in(dir.path(), "local").await;
    core.set_summary_backend_factory(Arc::new(|| {
        (Box::new(FakeBackend::failing(Failure::Unavailable)), None)
    }));

    let id = meeting_of(&core, 400).await;
    assert!(
        core.summarize_meeting(&id).await.is_err(),
        "a Backend that never answers must not produce a Summary"
    );

    let meeting = core
        .get_meeting(&id)
        .await
        .expect("get")
        .expect("the Meeting")
        .0;
    assert_eq!(meeting.summary, None, "and must store nothing");
}

#[tokio::test]
async fn the_suggested_title_survives_the_chunked_path() {
    // Ticket 02's behaviour has to hold on the many-chunk path too — the
    // title comes from the *final* markdown, not from the first chunk's.
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core_in(dir.path(), "local").await;
    let backend = Arc::new(std::sync::Mutex::new(Some(FakeBackend::scripted(
        BackendIdentity::LocalSidecar {
            model: "fake".into(),
        },
        vec![
            Response::Text("# A chunk heading\n\nBody.".into()),
            Response::Text("# Another chunk heading\n\nBody.".into()),
            Response::Text("# The whole meeting\n\nCombined body.".into()),
        ],
    ))));
    core.set_summary_backend_factory(Arc::new(move || {
        (
            Box::new(backend.lock().unwrap().take().expect("built once")),
            None,
        )
    }));

    let id = meeting_of(&core, 400).await;
    core.summarize_meeting(&id).await.expect("summarize");

    let meeting = core
        .get_meeting(&id)
        .await
        .expect("get")
        .expect("the Meeting")
        .0;
    assert_eq!(
        meeting.title.as_deref(),
        Some("The whole meeting"),
        "the name should come from the reduced Summary, not a chunk's"
    );
}

/// Why Summary is unavailable, not merely that it is.
///
/// These four used to be one sentence. With Local preselected and a
/// multi-gigabyte fetch running in the background, the likeliest case is a
/// model that is *arriving* — and "not available" for a model with a gigabyte
/// already on disk is the DECISIONS Q47 mistake: true, and useless.
mod why_summary_is_unavailable {
    use super::*;

    async fn reason(dir: &std::path::Path) -> String {
        let core = core_in(dir, "local").await;
        let id = meeting_of(&core, 3).await;
        core.summarize_meeting(&id)
            .await
            .expect_err("no model is staged, so this cannot succeed")
            .to_string()
    }

    #[tokio::test]
    async fn a_model_that_was_never_downloaded_says_so_and_names_its_size() {
        let dir = tempfile::tempdir().expect("tempdir");
        let message = reason(dir.path()).await;
        assert!(
            message.contains("not been downloaded") && message.contains("MB"),
            "an absent model should say it is absent and how big it is, got {message}"
        );
    }

    #[tokio::test]
    async fn a_model_still_arriving_says_how_far_along_it_is() {
        // The case Local-preselected makes common: a partial file on disk.
        let dir = tempfile::tempdir().expect("tempdir");
        let models = dir.path().join("History").join(".data").join("models");
        std::fs::create_dir_all(&models).expect("models dir");
        // At the *partial* path, which is where an in-flight download lives.
        // A short file at the final name is a different state — corrupt — and
        // the Downloader is right to distinguish them: one is arriving, the
        // other is wrong.
        let entry = evertranscript_core::models::registry::SUMMARY_DEFAULT;
        std::fs::write(
            models.join(format!("{}.partial", entry.filename)),
            vec![0u8; 4096],
        )
        .expect("partial");

        let message = reason(dir.path()).await;
        assert!(
            message.contains("still downloading") && message.contains("of"),
            "a partial model should report its progress rather than reading as absent, \
             got {message}"
        );
    }
}
