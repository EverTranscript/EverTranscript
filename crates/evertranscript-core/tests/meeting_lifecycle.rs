//! The Meeting lifecycle as a Client sees it: start, stop, retitle, search,
//! export, delete — and the Mirror that appears on disk alongside.
//!
//! These drive the real protocol over a real socket against a real database
//! and a real History folder. The observable outputs are exactly the ones the
//! PRD names: SQLite content, Mirror files, and protocol responses.

mod common;

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use evertranscript_core::Core;
use evertranscript_core::Server;
use evertranscript_core::client::CoreClient;
use evertranscript_core::transport;
use evertranscript_protocol::HistorySearchResponse;
use evertranscript_protocol::MeetingDeleteResponse;
use evertranscript_protocol::MeetingDetailResponse;
use evertranscript_protocol::MeetingExportResponse;
use evertranscript_protocol::MeetingListResponse;
use evertranscript_protocol::MeetingResponse;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct TestCore {
    socket_path: common::Endpoint,
    history_dir: PathBuf,
    core: Arc<Core>,
    shutdown: CancellationToken,
    _dir: tempfile::TempDir,
}

impl TestCore {
    async fn start() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = common::endpoint(dir.path());
        let history_dir = dir.path().join("History");

        let core = Core::with_history_dir_acknowledged(history_dir.clone()).expect("core");
        let listener = transport::bind(&socket_path).await.expect("bind");
        let (events_tx, events_rx) = mpsc::channel(64);
        let shutdown = CancellationToken::new();

        tokio::spawn(Server::new(Arc::clone(&core)).run(events_rx, shutdown.clone()));
        tokio::spawn(transport::serve(listener, events_tx, shutdown.clone()));
        tokio::spawn(
            core.mirror()
                .clone()
                .run(core.mirror_wake(), shutdown.clone()),
        );

        Self {
            socket_path,
            history_dir,
            core,
            shutdown,
            _dir: dir,
        }
    }

    async fn client(&self) -> CoreClient {
        let mut client = CoreClient::connect_to(&self.socket_path)
            .await
            .expect("connect");
        client
            .initialize("test-client", "0.0.0")
            .await
            .expect("initialize");
        client
    }

    /// Mirrors the Operator sees: markdown at the top of the History folder.
    fn mirrors(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.history_dir)
            .expect("read history")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.ends_with(".md"))
            .collect();
        names.sort();
        names
    }

    fn mirror_contents(&self, name: &str) -> String {
        std::fs::read_to_string(self.history_dir.join(name)).expect("read mirror")
    }
}

impl Drop for TestCore {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

#[tokio::test]
async fn recording_a_meeting_produces_a_mirror_on_disk() {
    let core = TestCore::start().await;
    let mut client = core.client().await;

    let started: MeetingResponse = client
        .request("meeting/start", Some(json!({ "detectedApp": "Zoom" })))
        .await
        .expect("start");
    assert!(started.meeting.ended_at.is_none());

    let stopped: MeetingResponse = client.request("meeting/stop", None).await.expect("stop");
    assert!(
        stopped.meeting.ended_at.is_some(),
        "stopping must persist the Meeting (story 5)"
    );

    let mirrors = core.mirrors();
    assert_eq!(mirrors.len(), 1, "one Meeting, one Mirror: {mirrors:?}");
    let name = &mirrors[0];
    assert!(
        name.ends_with(&format!(
            "-{}.md",
            &started.meeting.id.replace('-', "")[..8]
        )),
        "the filename must carry the id8: {name}"
    );
    assert!(
        name.contains("zoom"),
        "untitled Meetings slug on the app: {name}"
    );

    let body = core.mirror_contents(name);
    assert!(body.contains(&format!("id: {}", started.meeting.id)));
    assert!(body.contains("## Summary"));
    assert!(body.contains("## Transcript"));
}

#[tokio::test]
async fn a_second_recording_is_refused_while_one_is_running() {
    let core = TestCore::start().await;
    let mut client = core.client().await;

    client
        .request::<MeetingResponse>("meeting/start", None)
        .await
        .expect("first start");
    let error = client
        .request::<MeetingResponse>("meeting/start", None)
        .await
        .expect_err("a second concurrent Meeting must be refused");
    assert!(
        error.to_string().contains("already recording"),
        "the error should say why: {error}"
    );
}

#[tokio::test]
async fn retitling_renames_the_mirror_and_leaves_no_stale_copy() {
    let core = TestCore::start().await;
    let mut client = core.client().await;

    let started: MeetingResponse = client
        .request("meeting/start", Some(json!({ "detectedApp": "Zoom" })))
        .await
        .expect("start");
    client
        .request::<MeetingResponse>("meeting/stop", None)
        .await
        .expect("stop");
    let before = core.mirrors();
    assert_eq!(before.len(), 1);

    let retitled: MeetingResponse = client
        .request(
            "meeting/retitle",
            Some(json!({ "id": started.meeting.id, "title": "Frank / Jack Sync-Up" })),
        )
        .await
        .expect("retitle");

    let after = core.mirrors();
    assert_eq!(
        after.len(),
        1,
        "the old filename must be garbage-collected, not left behind: {after:?}"
    );
    assert!(
        after[0].contains("frank-jack-sync-up"),
        "the new name should carry the title: {after:?}"
    );
    assert_eq!(
        retitled.meeting.mirror_filename.as_deref(),
        Some(after[0].as_str())
    );

    let body = core.mirror_contents(&after[0]);
    assert!(body.contains("# Frank / Jack Sync-Up"));
}

#[tokio::test]
async fn transcript_segments_reach_the_mirror_and_full_text_search() {
    let core = TestCore::start().await;
    let mut client = core.client().await;

    let started: MeetingResponse = client
        .request("meeting/start", Some(json!({ "detectedApp": "Zoom" })))
        .await
        .expect("start");

    // Ticket 06's ASR is the real caller; the storage path is the same.
    core.core
        .append_segment(
            &started.meeting.id,
            evertranscript_protocol::AudioChannel::Mic,
            12_000,
            14_000,
            "we agreed to defer the hiring plan until October",
        )
        .await
        .expect("append");
    client
        .request::<MeetingResponse>("meeting/stop", None)
        .await
        .expect("stop");

    let detail: MeetingDetailResponse = client
        .request("meeting/get", Some(json!({ "id": started.meeting.id })))
        .await
        .expect("get");
    assert_eq!(detail.segments.len(), 1);

    let mirrors = core.mirrors();
    let body = core.mirror_contents(&mirrors[0]);
    assert!(
        body.contains("**You** (00:12) we agreed to defer the hiring plan until October"),
        "the transcript must render into the Mirror:\n{body}"
    );

    let found: HistorySearchResponse = client
        .request("history/search", Some(json!({ "query": "hiring plan" })))
        .await
        .expect("search");
    assert_eq!(found.results.len(), 1);
    assert_eq!(found.results[0].meeting.id, started.meeting.id);
}

#[tokio::test]
async fn export_renders_the_same_markdown_the_mirror_holds() {
    let core = TestCore::start().await;
    let mut client = core.client().await;

    let started: MeetingResponse = client
        .request("meeting/start", Some(json!({ "detectedApp": "Teams" })))
        .await
        .expect("start");
    client
        .request::<MeetingResponse>("meeting/stop", None)
        .await
        .expect("stop");

    let exported: MeetingExportResponse = client
        .request("meeting/export", Some(json!({ "id": started.meeting.id })))
        .await
        .expect("export");
    let mirrors = core.mirrors();
    assert_eq!(exported.markdown, core.mirror_contents(&mirrors[0]));
    assert_eq!(
        exported.mirror_path.as_deref(),
        Some(core.history_dir.join(&mirrors[0]).display().to_string()).as_deref()
    );
}

#[tokio::test]
async fn deleting_a_meeting_removes_its_rows_mirror_and_audio() {
    let core = TestCore::start().await;
    let mut client = core.client().await;

    let started: MeetingResponse = client
        .request("meeting/start", Some(json!({ "detectedApp": "Zoom" })))
        .await
        .expect("start");
    client
        .request::<MeetingResponse>("meeting/stop", None)
        .await
        .expect("stop");

    // Stand in for the audio ticket 03 will write.
    let audio_relative = format!(".data/audio/{}.m4a", &started.meeting.id[..8]);
    let audio_path = core.history_dir.join(&audio_relative);
    std::fs::create_dir_all(audio_path.parent().expect("parent")).expect("audio dir");
    std::fs::write(&audio_path, b"not really audio").expect("write audio");
    let id = started.meeting.id.clone();
    let relative = audio_relative.clone();
    core.core
        .store()
        .write(move |connection| {
            evertranscript_core::store::meetings::set_audio_path(connection, &id, &relative)
        })
        .await
        .expect("set audio path");

    assert_eq!(core.mirrors().len(), 1);
    assert!(audio_path.exists());

    let deleted: MeetingDeleteResponse = client
        .request("meeting/delete", Some(json!({ "id": started.meeting.id })))
        .await
        .expect("delete");
    assert!(deleted.deleted);

    assert!(core.mirrors().is_empty(), "the Mirror must be gone");
    assert!(!audio_path.exists(), "the audio must be gone");

    let listed: MeetingListResponse = client.request("meeting/list", None).await.expect("list");
    assert!(listed.meetings.is_empty(), "the rows must be gone");
}

#[tokio::test]
async fn the_history_folder_reads_as_meeting_notes() {
    let core = TestCore::start().await;
    let mut client = core.client().await;

    for app in ["Zoom", "Teams"] {
        client
            .request::<MeetingResponse>("meeting/start", Some(json!({ "detectedApp": app })))
            .await
            .expect("start");
        client
            .request::<MeetingResponse>("meeting/stop", None)
            .await
            .expect("stop");
    }

    // The whole point of hiding the machine store: everything visible at the
    // top level is a Meeting note (ADR-0035 as amended).
    let visible: Vec<String> = std::fs::read_dir(&core.history_dir)
        .expect("read history")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| !name.starts_with('.'))
        .collect();
    assert_eq!(
        visible.len(),
        2,
        "only Mirrors should be visible: {visible:?}"
    );
    assert!(visible.iter().all(|name| name.ends_with(".md")));

    // And the store is where it belongs, hidden.
    assert!(Path::new(&core.history_dir).join(".data").is_dir());
    assert!(core.history_dir.join(".data/EverTranscript.db").is_file());
}
