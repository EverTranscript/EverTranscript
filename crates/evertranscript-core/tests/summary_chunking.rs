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
use evertranscript_core::Server;
use evertranscript_core::client::CoreClient;
use evertranscript_core::store::meetings;
use evertranscript_core::summary::Backend;
use evertranscript_core::summary::BackendError;
use evertranscript_core::summary::BackendIdentity;
use evertranscript_core::summary::Cancel;
use evertranscript_core::summary::Request;
use evertranscript_core::summary::fake::Failure;
use evertranscript_core::summary::fake::FakeBackend;
use evertranscript_core::summary::fake::Response;
use evertranscript_core::transport;
use evertranscript_protocol::AudioChannel;
use evertranscript_protocol::MeetingResponse;
use evertranscript_protocol::SettingsSetParams;
use tokio_util::sync::CancellationToken;

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
    // An empty script, not the live factory this would otherwise default to.
    // Nothing here is synthesised through a source — the transcript is written
    // straight to the store — but `meeting_of` still goes through
    // `start_meeting`, and that opens a real microphone on the machine running
    // the test.
    core.set_source_factory(Arc::new(|| {
        Box::new(evertranscript_core::audio::fixture::FixtureSource::new(
            Vec::new(),
        ))
    }))
    .await;
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

/// A Backend that holds its first answer until the test lets it go, so the
/// test can act while a Summary is being generated.
struct Held {
    inner: FakeBackend,
    started: Option<std::sync::mpsc::Sender<()>>,
    release: Option<std::sync::mpsc::Receiver<()>>,
}

impl Backend for Held {
    fn generate(&mut self, request: &Request, cancel: &Cancel) -> Result<String, BackendError> {
        if let (Some(started), Some(release)) = (self.started.take(), self.release.take()) {
            let _ = started.send(());
            let _ = release.recv();
        }
        self.inner.generate(request, cancel)
    }

    fn identity(&self) -> BackendIdentity {
        self.inner.identity()
    }
}

#[tokio::test]
async fn switching_the_knob_mid_generation_leaves_the_run_alone() {
    // Ticket 07: switching the Knob mid-generation does not corrupt the
    // Meeting being summarized. Cloud to local is the direction where a
    // corrupt record is within reach, because the local Backend is already
    // in the run as the fallback: a Knob read per chunk would hand it the
    // remaining chunks under the cloud's label. The run finishes where it
    // started, and the switch is what the next run uses (Q109).
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core_in(dir.path(), "openai").await;
    let (started, reached) = std::sync::mpsc::channel();
    let (release, held) = std::sync::mpsc::channel();
    let cloud = FakeBackend::cloud(
        "OpenAI",
        vec![Response::Text("# Cloud answered\n\nBody.".into())],
    );
    let cloud_prompts = cloud.prompts();
    let local = FakeBackend::returning("# Local answered\n\nBody.");
    let local_prompts = local.prompts();
    let cloud = Held {
        inner: cloud,
        started: Some(started),
        release: Some(held),
    };
    let parts = Arc::new(std::sync::Mutex::new(Some((cloud, local))));
    let built = Arc::new(std::sync::Mutex::new(0usize));
    let counter = Arc::clone(&built);
    core.set_summary_backend_factory(Arc::new(move || {
        *counter.lock().unwrap() += 1;
        let (cloud, local) = parts.lock().unwrap().take().expect("built once");
        (Box::new(cloud), Some(Box::new(local)))
    }));

    let id = meeting_of(&core, 400).await;
    let run = tokio::spawn({
        let core = Arc::clone(&core);
        let id = id.clone();
        async move { core.summarize_meeting(&id).await }
    });
    tokio::task::spawn_blocking(move || reached.recv_timeout(std::time::Duration::from_secs(30)))
        .await
        .expect("join")
        .expect("the run should have reached its Backend");
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        core.update_settings(SettingsSetParams {
            summary_backend: Some("local".to_string()),
            ..Default::default()
        }),
    )
    .await
    .expect("switching the Knob must not wait for the run")
    .expect("switch");
    release.send(()).expect("release");

    let markdown = run.await.expect("join").expect("summarize");
    assert!(markdown.contains("Cloud answered") && !markdown.contains("Local answered"));
    assert!(
        cloud_prompts.lock().unwrap().len() > 2,
        "the Backend the run started on should have served every chunk and the reduce"
    );
    assert!(
        local_prompts.lock().unwrap().is_empty(),
        "the switch reached the run in progress"
    );
    assert_eq!(
        *built.lock().unwrap(),
        1,
        "the Backends were built again mid-run"
    );
    let meeting = core
        .get_meeting(&id)
        .await
        .expect("get")
        .expect("the Meeting")
        .0;
    let label = meeting.summary_backend.expect("a Backend label");
    assert!(label.starts_with("OpenAI"), "labelled {label}");
    assert_eq!(
        core.settings().await.summary_backend.as_deref(),
        Some("local"),
        "the switch itself should hold for the next run"
    );
}

#[tokio::test]
async fn a_summary_being_generated_does_not_hold_up_other_clients() {
    // One loop answers every Client and forwards every notification, and it
    // used to wait out a Summary before reading the next request: for the
    // minutes a local run takes, even a Stop from the window went unanswered.
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core_in(dir.path(), "local").await;
    let (started, reached) = std::sync::mpsc::channel();
    let (release, held) = std::sync::mpsc::channel();
    let backend = Arc::new(std::sync::Mutex::new(Some(Held {
        inner: FakeBackend::returning("# Held back\n\nBody."),
        started: Some(started),
        release: Some(held),
    })));
    core.set_summary_backend_factory(Arc::new(move || {
        let backend = backend.lock().unwrap().take().expect("built once");
        (Box::new(backend), None)
    }));
    let id = meeting_of(&core, 3).await;

    let socket = dir.path().join("s");
    let listener = transport::bind(&socket).await.expect("bind");
    let (events_tx, events_rx) = tokio::sync::mpsc::channel(64);
    let shutdown = CancellationToken::new();
    tokio::spawn(Server::new(Arc::clone(&core)).run(events_rx, shutdown.clone()));
    tokio::spawn(transport::serve(listener, events_tx, shutdown.clone()));

    let mut window = CoreClient::connect_to(&socket).await.expect("connect");
    window
        .initialize("window", "0.0.0")
        .await
        .expect("initialize");
    let run = tokio::spawn(async move {
        window
            .request::<MeetingResponse>("summary/generate", Some(serde_json::json!({ "id": id })))
            .await
    });
    tokio::task::spawn_blocking(move || reached.recv_timeout(std::time::Duration::from_secs(30)))
        .await
        .expect("join")
        .expect("the run should have reached its Backend");

    let mut other = CoreClient::connect_to(&socket).await.expect("connect");
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        other.initialize("cli", "0.0.0").await.expect("initialize");
        other.status().await.expect("status");
    })
    .await
    .expect("another Client should be answered while a Summary is generated");

    release.send(()).expect("release");
    let answered = run.await.expect("join").expect("summary/generate");
    assert!(
        answered
            .meeting
            .summary
            .is_some_and(|summary| summary.contains("Held back")),
        "the Client that asked should still get the Summary"
    );
    shutdown.cancel();
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
async fn an_item_crediting_the_unnamed_placeholder_is_left_out_and_said_so() {
    // The model files action items under "Participant", the placeholder
    // `render_transcript` gives every unnamed Speaker on the system channel.
    // The row names nobody and `verify` can only check it against the pooled
    // speech of the whole room, so it goes — and a table one row shorter is a
    // changed record, so the record says so (Q123).
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core_in(dir.path(), "local").await;
    core.set_summary_backend_factory(Arc::new(|| {
        (
            Box::new(FakeBackend::returning(
                "# The quarterly plan\n\nDiscussed the plan.\n\n## Action items\n\n\
                 | Who | What | When | Said at |\n\
                 |---|---|---|---|\n\
                 | You | discussed the quarterly plan | next week | 00:00:05 |\n\
                 | Participant | wire the retainer to account 4471 | Friday | 00:00:10 |\n",
            )),
            None,
        )
    }));

    // Short enough to be one chunk, so the count is the row rather than the
    // row once per chunk.
    let id = meeting_of(&core, 20).await;
    core.summarize_meeting(&id).await.expect("summarize");

    let meeting = core
        .get_meeting(&id)
        .await
        .expect("get")
        .expect("the Meeting")
        .0;
    let summary = meeting.summary.expect("a Summary");
    assert!(
        !summary.contains("4471"),
        "the placeholder's item must not reach the record: {summary}"
    );
    assert!(
        summary.contains("discussed the quarterly plan"),
        "the item crediting a person must survive: {summary}"
    );
    let gaps = meeting
        .summary_gaps
        .expect("a Summary that lost a row must say so");
    assert!(
        gaps.contains("1 action item was left out"),
        "the note should say what happened, got {gaps}"
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

/// A fresh install has a working Summary configuration without the Operator
/// having to research a choice they have no basis for (ADR-0013 as amended).
mod preselection {
    use super::*;

    async fn backend_of(core: &Core) -> Option<String> {
        core.settings().await.summary_backend
    }

    #[tokio::test]
    async fn a_fresh_install_gets_local_without_choosing_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");
        assert_eq!(backend_of(&core).await, None, "starts unchosen");

        core.preselect_local_backend().await.expect("preselect");
        assert_eq!(backend_of(&core).await.as_deref(), Some("local"));
    }

    #[tokio::test]
    async fn an_operator_who_chose_cloud_is_never_reset_by_an_upgrade() {
        // The failure that would matter: installing a newer version silently
        // moving someone off the Backend they deliberately chose.
        let dir = tempfile::tempdir().expect("tempdir");
        let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");
        core.update_settings(SettingsSetParams {
            summary_backend: Some("openai".to_string()),
            summary_cloud_warning_accepted: Some(true),
            ..Default::default()
        })
        .await
        .expect("choose cloud");

        core.preselect_local_backend().await.expect("preselect");
        assert_eq!(
            backend_of(&core).await.as_deref(),
            Some("openai"),
            "a deliberate choice must survive"
        );
    }

    #[tokio::test]
    async fn preselecting_twice_changes_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");
        core.preselect_local_backend().await.expect("first");
        core.preselect_local_backend().await.expect("second");
        assert_eq!(backend_of(&core).await.as_deref(), Some("local"));
    }
}
