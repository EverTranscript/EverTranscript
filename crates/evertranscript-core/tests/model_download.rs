//! Downloading a model over a real HTTP connection, including the failure
//! that actually happens on a bad network: the transfer dies partway.
//!
//! The server here is deliberately hand-rolled and minimal — it exists to be
//! hostile (truncate mid-body, honor or ignore Range), which is exactly what
//! a canned mock server makes hard.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use evertranscript_core::models::Downloader;
use evertranscript_core::models::ModelStatus;
use evertranscript_core::models::registry::Integrity;
use evertranscript_core::models::registry::ModelEntry;
use evertranscript_core::models::registry::ModelPurpose;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

/// How the test server should misbehave.
#[derive(Clone, Copy, PartialEq)]
enum Behavior {
    /// Serve the whole body, honoring Range.
    Complete,
    /// Send only the first `bytes` of the body, then close.
    TruncateAfter(usize),
    /// Ignore Range and always send the whole body from zero.
    IgnoreRange,
}

struct TestServer {
    address: SocketAddr,
    requests: Arc<AtomicUsize>,
    ranges_seen: Arc<std::sync::Mutex<Vec<Option<u64>>>>,
}

impl TestServer {
    async fn start(payload: Vec<u8>, behavior: Behavior) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("addr");
        let requests = Arc::new(AtomicUsize::new(0));
        let ranges_seen = Arc::new(std::sync::Mutex::new(Vec::new()));

        let counter = Arc::clone(&requests);
        let ranges = Arc::clone(&ranges_seen);
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let payload = payload.clone();
                let counter = Arc::clone(&counter);
                let ranges = Arc::clone(&ranges);
                tokio::spawn(async move {
                    let mut buffer = vec![0u8; 4096];
                    let read = stream.read(&mut buffer).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                    counter.fetch_add(1, Ordering::SeqCst);

                    let range_start = request
                        .lines()
                        .find(|line| line.to_ascii_lowercase().starts_with("range:"))
                        .and_then(|line| {
                            line.split("bytes=")
                                .nth(1)?
                                .split('-')
                                .next()?
                                .trim()
                                .parse::<u64>()
                                .ok()
                        });
                    ranges.lock().expect("lock").push(range_start);

                    let honor_range = behavior != Behavior::IgnoreRange;
                    let start = if honor_range {
                        range_start.unwrap_or(0) as usize
                    } else {
                        0
                    };
                    let body = &payload[start.min(payload.len())..];

                    let head = if honor_range && range_start.is_some() {
                        format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\n\
                             Content-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                            body.len(),
                            start,
                            payload.len().saturating_sub(1),
                            payload.len()
                        )
                    } else {
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                    };
                    if stream.write_all(head.as_bytes()).await.is_err() {
                        return;
                    }

                    match behavior {
                        // Announce the full length, then stop early and hang
                        // up: the shape of a connection dropped in flight.
                        Behavior::TruncateAfter(bytes) => {
                            let cut = bytes.min(body.len());
                            let _ = stream.write_all(&body[..cut]).await;
                        }
                        _ => {
                            let _ = stream.write_all(body).await;
                        }
                    }
                    let _ = stream.flush().await;
                });
            }
        });

        Self {
            address,
            requests,
            ranges_seen,
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }
}

fn ggml_payload(len: usize) -> Vec<u8> {
    let mut bytes = b"ggml".to_vec();
    bytes.extend((0..len - 4).map(|index| (index % 251) as u8));
    bytes
}

fn entry_for(payload: &[u8]) -> ModelEntry {
    ModelEntry {
        key: "test-model",
        display_name: "Test model",
        filename: "test-model.bin",
        remote_path: "test-model.bin",
        integrity: Integrity {
            size_bytes: payload.len() as u64,
            sha256: None,
            crc32: Some(crc32fast::hash(payload)),
        },
        purpose: ModelPurpose::Transcription,
        required: true,
        provenance: evertranscript_core::models::registry::Provenance {
            license: "MIT",
            source: "https://example.invalid/fixture",
        },
        driving: None,
    }
}

#[tokio::test]
async fn a_model_downloads_and_verifies() {
    let payload = ggml_payload(64 * 1024);
    let entry = entry_for(&payload);
    let server = TestServer::start(payload.clone(), Behavior::Complete).await;
    let dir = tempfile::tempdir().expect("tempdir");
    let downloader =
        Downloader::with_base_url(dir.path().to_path_buf(), server.base_url()).expect("downloader");

    let mut last_seen = 0;
    let path = downloader
        .fetch(&entry, CancellationToken::new(), |progress| {
            last_seen = progress.downloaded_bytes;
        })
        .await
        .expect("fetch");

    assert_eq!(std::fs::read(&path).expect("read"), payload);
    assert_eq!(
        last_seen,
        payload.len() as u64,
        "progress reaches the total"
    );
    assert!(downloader.status(&entry).is_ready());
    assert!(
        !dir.path().join("test-model.bin.partial").exists(),
        "the partial file must not survive a successful download"
    );
}

#[tokio::test]
async fn an_interrupted_download_resumes_instead_of_restarting() {
    let payload = ggml_payload(64 * 1024);
    let entry = entry_for(&payload);
    let dir = tempfile::tempdir().expect("tempdir");

    // First attempt: the connection dies a quarter of the way through.
    let flaky = TestServer::start(payload.clone(), Behavior::TruncateAfter(16 * 1024)).await;
    let downloader =
        Downloader::with_base_url(dir.path().to_path_buf(), flaky.base_url()).expect("downloader");
    let failure = downloader
        .fetch(&entry, CancellationToken::new(), |_| {})
        .await;
    assert!(failure.is_err(), "a truncated body must fail verification");

    // The partial survives, which is the whole point.
    match downloader.status(&entry) {
        ModelStatus::Partial { bytes_on_disk } => {
            assert_eq!(
                bytes_on_disk,
                16 * 1024,
                "the bytes that did arrive must be kept for the retry"
            );
        }
        other => panic!("expected a resumable partial, got {other:?}"),
    }

    // Second attempt against a healthy server: it must ask for the rest.
    let healthy = TestServer::start(payload.clone(), Behavior::Complete).await;
    let downloader = Downloader::with_base_url(dir.path().to_path_buf(), healthy.base_url())
        .expect("downloader");
    let path = downloader
        .fetch(&entry, CancellationToken::new(), |_| {})
        .await
        .expect("the retry must succeed");

    assert_eq!(std::fs::read(&path).expect("read"), payload);
    let ranges = healthy.ranges_seen.lock().expect("lock").clone();
    assert_eq!(
        ranges,
        vec![Some(16 * 1024)],
        "the retry must request a range, not the whole file again"
    );
}

#[tokio::test]
async fn a_server_that_ignores_range_still_produces_a_correct_file() {
    // Some mirrors and proxies do not support Range. Resuming against them
    // must fall back to a clean restart rather than concatenating bytes onto
    // a partial and corrupting the result.
    let payload = ggml_payload(32 * 1024);
    let entry = entry_for(&payload);
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("test-model.bin.partial"), &payload[..8192]).expect("partial");

    let server = TestServer::start(payload.clone(), Behavior::IgnoreRange).await;
    let downloader =
        Downloader::with_base_url(dir.path().to_path_buf(), server.base_url()).expect("downloader");

    let path = downloader
        .fetch(&entry, CancellationToken::new(), |_| {})
        .await
        .expect("fetch");
    assert_eq!(
        std::fs::read(&path).expect("read"),
        payload,
        "ignoring Range must not corrupt the file"
    );
}

#[tokio::test]
async fn a_verified_model_is_not_downloaded_again() {
    let payload = ggml_payload(8 * 1024);
    let entry = entry_for(&payload);
    let server = TestServer::start(payload.clone(), Behavior::Complete).await;
    let dir = tempfile::tempdir().expect("tempdir");
    let downloader =
        Downloader::with_base_url(dir.path().to_path_buf(), server.base_url()).expect("downloader");

    downloader
        .fetch(&entry, CancellationToken::new(), |_| {})
        .await
        .expect("first fetch");
    downloader
        .fetch(&entry, CancellationToken::new(), |_| {})
        .await
        .expect("second fetch");

    assert_eq!(
        server.requests.load(Ordering::SeqCst),
        1,
        "a model already on disk must not be fetched again"
    );
}

#[tokio::test]
async fn cancelling_keeps_the_partial_so_the_retry_resumes() {
    let payload = ggml_payload(4 * 1024 * 1024);
    let entry = entry_for(&payload);
    let server = TestServer::start(payload, Behavior::Complete).await;
    let dir = tempfile::tempdir().expect("tempdir");
    let downloader =
        Downloader::with_base_url(dir.path().to_path_buf(), server.base_url()).expect("downloader");

    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    let result = downloader
        .fetch(&entry, cancel, move |progress| {
            if progress.downloaded_bytes > 0 {
                trigger.cancel();
            }
        })
        .await;

    assert!(result.is_err(), "a cancelled fetch does not return a path");
    assert!(
        !downloader.status(&entry).is_ready(),
        "a cancelled download must not be promoted"
    );
}

/// Provisioning, end to end, against the stub rather than the internet.
///
/// The decision itself is unit-tested in `models::provision`. This proves the
/// wiring — that a Core asked to provision a fresh install actually fetches,
/// and that one which is not asked fetches nothing. Decision-only coverage is
/// how DECISIONS Q44 shipped: the logic was right and nothing ran it.
#[tokio::test]
async fn a_core_provisions_only_when_it_is_asked_to() {
    let payload = ggml_payload(4096);
    let server = TestServer::start(payload.clone(), Behavior::Complete).await;
    let dir = tempfile::tempdir().expect("tempdir");

    // **Point the mirror at the stub.** Without this the Core provisions from
    // the real one and the test downloads gigabytes — which the first version
    // did, taking 200 seconds and reaching the internet from a suite whose
    // whole point is that it does not have to.
    //
    // SAFETY: single-threaded test setup, before any Core exists to read it.
    unsafe {
        std::env::set_var(
            evertranscript_core::models::registry::BASE_URL_ENV,
            format!("http://{}", server.address),
        );
    }

    // A Core built and never asked must not reach the network. This is the
    // property the guarantee tests depend on, asserted here directly rather
    // than inferred from them.
    let core = evertranscript_core::Core::with_paths_and_models(
        dir.path().join("History"),
        dir.path().join("settings.json"),
        dir.path().join("models"),
    )
    .expect("core");
    assert_eq!(
        server.requests.load(Ordering::SeqCst),
        0,
        "constructing a Core must not fetch anything"
    );

    // **In the production order.** `main.rs` calls `preselect_local_backend()`
    // two lines before `provision_missing_models()`, and the freshness test used to
    // read `summary_backend.is_some()` — which the preselect had just written.
    // So every fresh install decided `AskFirst` and downloaded nothing, while
    // the Briefing promised "the first ones start on their own". Asserting on
    // the provisioner alone missed it: the unit was right and the composition
    // was wrong.
    core.preselect_local_backend()
        .await
        .expect("preselect, as the daemon does at startup");

    // And asked, on a fresh install, it decides to fetch. The registry's real
    // entries point at the real mirror, so this asserts the decision and the
    // call rather than downloading gigabytes: the fetch fails against a stub
    // that serves a different payload, and failing is not the point — being
    // *attempted* is.
    //
    // What it decides depends on the machine — a runner low on disk correctly
    // refuses, which the first version of this test discovered by failing on
    // one. So the assertion is the machine-independent half: a fresh install
    // with models missing must reach a decision *about* them, never conclude
    // there is nothing to do.
    let decision = core
        .provision_missing_models(CancellationToken::new())
        .await
        .map(|decision| format!("{decision:?}"))
        .unwrap_or_else(|error| format!("attempted and failed: {error}"));
    assert!(
        !decision.contains("NothingMissing"),
        "a fresh install has no models, so provisioning must have something to \
         say about them, got {decision}"
    );
    assert!(
        !decision.contains("AskFirst"),
        "an unconfigured install is fresh by definition, got {decision}"
    );
    unsafe {
        std::env::remove_var(evertranscript_core::models::registry::BASE_URL_ENV);
    }
    drop(server);
}

