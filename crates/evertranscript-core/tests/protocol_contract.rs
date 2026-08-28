//! The protocol contract as a Client sees it.
//!
//! These tests drive the real transport and the real server over a real
//! socket — the observable surface, not the internals. Per the PRD's testing
//! philosophy, the protocol *is* the tested seam for the Core's control
//! plane, which is why the Electron Client can stay thin.

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Arc;

use evertranscript_core::client::CoreClient;
use evertranscript_core::transport;
use evertranscript_core::Core;
use evertranscript_core::Server;
use evertranscript_protocol::error_codes;
use evertranscript_protocol::CoreState;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// A Core listening on a private socket, torn down when dropped.
struct TestCore {
    socket_path: PathBuf,
    shutdown: CancellationToken,
    _dir: tempfile::TempDir,
}

impl TestCore {
    async fn start() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        // Short path: unix socket paths are length-limited.
        let socket_path = dir.path().join("s");
        let history_dir = dir.path().join("History");

        let core = Core::with_history_dir(history_dir).expect("core");
        let listener = transport::bind(&socket_path).await.expect("bind");
        let (events_tx, events_rx) = mpsc::channel(64);
        let shutdown = CancellationToken::new();

        let server = Server::new(Arc::clone(&core));
        tokio::spawn(server.run(events_rx, shutdown.clone()));
        tokio::spawn(transport::serve(listener, events_tx, shutdown.clone()));

        Self {
            socket_path,
            shutdown,
            _dir: dir,
        }
    }

    async fn client(&self) -> CoreClient {
        CoreClient::connect_to(&self.socket_path)
            .await
            .expect("connect")
    }
}

impl Drop for TestCore {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

#[tokio::test]
async fn a_client_initializes_and_reads_status() {
    let core = TestCore::start().await;
    let mut client = core.client().await;

    let initialize = client
        .initialize("test-client", "0.0.0")
        .await
        .expect("initialize");
    assert_eq!(initialize.server_info.name, "evertranscript-core");
    assert_eq!(
        initialize.server_info.protocol_version,
        evertranscript_protocol::PROTOCOL_VERSION
    );

    let status = client.status().await.expect("status");
    assert_eq!(status.version, evertranscript_protocol::VERSION);
    assert_eq!(status.pid, std::process::id());
    assert_eq!(status.state, CoreState::Idle);
    assert!(
        status.incomplete_copy_warning.is_none(),
        "a fresh History folder is not an incomplete copy"
    );
}

#[tokio::test]
async fn requests_before_initialize_are_refused() {
    let core = TestCore::start().await;
    let mut client = core.client().await;

    let error = client.status().await.expect_err("status must be refused");
    let message = error.to_string();
    assert!(
        message.contains(&error_codes::NOT_INITIALIZED.to_string()),
        "expected NOT_INITIALIZED, got: {message}"
    );
}

#[tokio::test]
async fn initializing_twice_is_refused() {
    let core = TestCore::start().await;
    let mut client = core.client().await;

    client
        .initialize("test-client", "0.0.0")
        .await
        .expect("first initialize");
    let error = client
        .initialize("test-client", "0.0.0")
        .await
        .expect_err("second initialize must be refused");
    assert!(
        error
            .to_string()
            .contains(&error_codes::ALREADY_INITIALIZED.to_string()),
        "expected ALREADY_INITIALIZED, got: {error}"
    );
}

#[tokio::test]
async fn unknown_methods_are_reported_not_fatal() {
    let core = TestCore::start().await;
    let mut client = core.client().await;
    client
        .initialize("test-client", "0.0.0")
        .await
        .expect("initialize");

    let error = client
        .request::<serde_json::Value>("nope/notAMethod", None)
        .await
        .expect_err("unknown method must error");
    assert!(
        error
            .to_string()
            .contains(&error_codes::METHOD_NOT_FOUND.to_string()),
        "expected METHOD_NOT_FOUND, got: {error}"
    );

    // The connection survives: a bad request is not a fatal protocol error.
    let status = client.status().await.expect("status still works");
    assert_eq!(status.state, CoreState::Idle);
}

#[tokio::test]
async fn many_clients_attach_concurrently() {
    let core = TestCore::start().await;

    // The Client and the CLI are attached at the same time in normal use;
    // the Core must serve both without either noticing (ADR-0026).
    let mut clients = Vec::new();
    for index in 0..5 {
        let mut client = core.client().await;
        client
            .initialize(&format!("client-{index}"), "0.0.0")
            .await
            .expect("initialize");
        clients.push(client);
    }
    for client in &mut clients {
        let status = client.status().await.expect("status");
        assert_eq!(status.pid, std::process::id());
    }
}

#[tokio::test]
async fn a_disconnecting_client_does_not_disturb_the_others() {
    let core = TestCore::start().await;

    let mut staying = core.client().await;
    staying
        .initialize("staying", "0.0.0")
        .await
        .expect("initialize");

    {
        let mut leaving = core.client().await;
        leaving
            .initialize("leaving", "0.0.0")
            .await
            .expect("initialize");
        leaving.status().await.expect("status");
    } // dropped: the connection closes here

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let status = staying.status().await.expect("the survivor still works");
    assert_eq!(status.state, CoreState::Idle);
}

#[tokio::test]
async fn a_second_core_refuses_to_bind_a_live_socket() {
    let core = TestCore::start().await;

    let error = transport::bind(&core.socket_path)
        .await
        .expect_err("a second Core must refuse to bind");
    assert_eq!(
        error.kind(),
        std::io::ErrorKind::AddrInUse,
        "expected AddrInUse, got: {error}"
    );
}

#[tokio::test]
async fn a_stale_socket_file_is_cleaned_up() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket_path = dir.path().join("s");

    // A socket file left behind by a Core that died without unlinking: bind
    // it with raw tokio (which does not unlink on drop) and let it go, so the
    // file survives with nothing listening behind it.
    {
        let _raw = tokio::net::UnixListener::bind(&socket_path).expect("raw bind");
    }
    assert!(
        socket_path.exists(),
        "the stale socket file should survive the dead listener"
    );

    // Binding again must clean it up rather than refuse.
    let _listener = transport::bind(&socket_path)
        .await
        .expect("a stale socket must not block startup");
}

#[tokio::test]
async fn an_incomplete_copy_is_reported_in_status() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket_path = dir.path().join("s");
    let history_dir = dir.path().join("History");

    // Mirrors copied without the hidden machine store (ADR-0035).
    std::fs::create_dir_all(&history_dir).expect("create history");
    std::fs::write(
        history_dir.join("2026-08-27-zoom-a3f8c21b.md"),
        "# A meeting\n",
    )
    .expect("write mirror");

    let core = Core::with_history_dir(history_dir).expect("core");
    let listener = transport::bind(&socket_path).await.expect("bind");
    let (events_tx, events_rx) = mpsc::channel(64);
    let shutdown = CancellationToken::new();
    tokio::spawn(Server::new(Arc::clone(&core)).run(events_rx, shutdown.clone()));
    tokio::spawn(transport::serve(listener, events_tx, shutdown.clone()));

    let mut client = CoreClient::connect_to(&socket_path).await.expect("connect");
    client
        .initialize("test-client", "0.0.0")
        .await
        .expect("initialize");
    let status = client.status().await.expect("status");

    // Creating the missing store must not erase the evidence that the copy
    // was partial: the Operator believes they moved their whole History and
    // needs to be told that audio and Voiceprints were left behind.
    let warning = status
        .incomplete_copy_warning
        .expect("an incomplete copy must be reported in status");
    assert!(
        warning.contains("incomplete copy"),
        "the warning should say what happened: {warning}"
    );
    shutdown.cancel();
}
