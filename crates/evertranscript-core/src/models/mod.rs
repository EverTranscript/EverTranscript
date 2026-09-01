//! Fetching and verifying the models the Core needs.
//!
//! Onboarding downloads roughly a gigabyte, sometimes over a network that
//! drops. Everything here exists so that is survivable: partial files resume
//! instead of restarting, a dead connection that never errors is caught by a
//! stall timeout rather than hanging forever, and nothing is promoted into
//! place until its bytes verify.

/// Free bytes on the volume holding `path`, or `0` when it cannot be read.
///
/// Zero refuses the download rather than starting one that cannot finish: a
/// probe that fails should cost the convenience, never the disk.
pub fn free_space_bytes(path: &std::path::Path) -> u64 {
    // Walk up to something that exists — the models directory may not have
    // been created yet on the fresh install this is about.
    let mut probe = path;
    loop {
        if probe.exists() {
            break;
        }
        match probe.parent() {
            Some(parent) => probe = parent,
            None => return 0,
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(c_path) = std::ffi::CString::new(probe.as_os_str().as_bytes()) else {
            return 0;
        };
        // SAFETY: a zeroed statvfs is a valid initial value, and the path is a
        // NUL-terminated C string that outlives the call.
        unsafe {
            let mut stat: libc::statvfs = std::mem::zeroed();
            if libc::statvfs(c_path.as_ptr(), &mut stat) != 0 {
                return 0;
            }
            (stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64)
        }
    }
    #[cfg(not(unix))]
    {
        let _ = probe;
        // Windows reports this through GetDiskFreeSpaceEx; until that is
        // wired, refusing to guess is better than guessing generously.
        u64::MAX
    }
}

/// Model files this build no longer knows how to load.
///
/// **By exact filename, never a glob.** Application Support holds the
/// Operator's models, and a pattern that swept up "anything that looks like an
/// old summary model" would eventually sweep up something of theirs. A list
/// that must be edited by hand is the point: removing a file from someone's
/// disk should be a deliberate act recorded in a diff.
const SUPERSEDED: &[&str] = &[
    // Replaced by Qwen3-4B. Half a gigabyte no build can load.
    "summary-qwen2.5-0.5b-instruct-q4_k_m.gguf",
];

/// Deletes models this build superseded, returning what it removed.
///
/// Application Support is the re-creatable half of the product — models were
/// never part of the portable unit, and the Homebrew cask already deletes this
/// directory on uninstall for the same reason. **History is never touched**:
/// that is the Operator's record and the thing this product exists to keep.
pub fn remove_superseded(models_dir: &std::path::Path) -> Vec<String> {
    let mut removed = Vec::new();
    for filename in SUPERSEDED {
        // Guard against the list ever naming something still registered — a
        // future edit that supersedes a model and forgets to unregister it
        // would otherwise delete the file the product is about to load.
        if registry::ALL
            .iter()
            .any(|entry| entry.filename == *filename)
        {
            continue;
        }
        let path = models_dir.join(filename);
        if path.is_file() && std::fs::remove_file(&path).is_ok() {
            removed.push((*filename).to_string());
        }
    }
    removed
}

#[cfg(test)]
mod superseded_tests {
    use super::*;

    #[test]
    fn a_superseded_model_is_removed_and_nothing_else_is() {
        let dir = tempfile::tempdir().expect("tempdir");
        let models = dir.path();
        for name in [
            "summary-qwen2.5-0.5b-instruct-q4_k_m.gguf",
            "ggml-large-v3-turbo-q8_0.bin",
            "something-the-operator-put-here.gguf",
        ] {
            std::fs::write(models.join(name), b"x").expect("write");
        }

        let removed = remove_superseded(models);
        assert_eq!(removed, vec!["summary-qwen2.5-0.5b-instruct-q4_k_m.gguf"]);
        assert!(
            !models
                .join("summary-qwen2.5-0.5b-instruct-q4_k_m.gguf")
                .exists()
        );
        assert!(
            models.join("ggml-large-v3-turbo-q8_0.bin").exists(),
            "a registered model must survive"
        );
        assert!(
            models.join("something-the-operator-put-here.gguf").exists(),
            "a file this product did not put here is not ours to delete"
        );
    }

    #[test]
    fn an_install_that_never_had_it_is_unaffected() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(remove_superseded(dir.path()).is_empty());
    }

    #[test]
    fn nothing_still_registered_is_ever_in_the_superseded_list() {
        // The guard that matters most: superseding a model without
        // unregistering it would delete the file the product is about to load.
        for filename in SUPERSEDED {
            assert!(
                !registry::ALL
                    .iter()
                    .any(|entry| entry.filename == *filename),
                "{filename} is both registered and superseded"
            );
        }
    }
}

pub mod provision;
pub mod registry;

use std::io::SeekFrom;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use futures_util::StreamExt;
use registry::Integrity;
use registry::ModelEntry;
use sha2::Digest;
use tokio::io::AsyncSeekExt;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;
use tracing::debug;
use tracing::info;
use tracing::warn;

/// A single chunk must arrive within this window. Without it, a connection
/// that goes silent without erroring hangs the download indefinitely — the
/// failure mode a plain overall timeout does not catch.
const CHUNK_STALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Suffix for in-progress downloads. Kept on cancellation so the next
/// attempt resumes rather than starting the gigabyte again.
const PARTIAL_SUFFIX: &str = "partial";

/// What the Core knows about one model on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelStatus {
    /// Nothing on disk.
    Missing,
    /// A resumable partial download.
    Partial { bytes_on_disk: u64 },
    /// Present but wrong. Deleted and re-fetched rather than used.
    Corrupted { reason: String },
    /// Present and verified.
    Ready { path: PathBuf },
}

impl ModelStatus {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }
}

/// Progress for the UI: bytes so far and the total when known.
#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
}

impl Progress {
    pub fn fraction(&self) -> f64 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        (self.downloaded_bytes as f64 / self.total_bytes as f64).clamp(0.0, 1.0)
    }
}

pub struct Downloader {
    models_dir: PathBuf,
    base_url: String,
    client: reqwest::Client,
}

impl Downloader {
    pub fn new(models_dir: PathBuf) -> Result<Self> {
        Self::with_base_url(models_dir, registry::base_url())
    }

    pub fn with_base_url(models_dir: PathBuf, base_url: String) -> Result<Self> {
        let client = reqwest::Client::builder()
            .tcp_nodelay(true)
            .connect_timeout(std::time::Duration::from_secs(30))
            .user_agent(concat!("evertranscript/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("building the HTTP client")?;
        Ok(Self {
            models_dir,
            base_url,
            client,
        })
    }

    pub fn models_dir(&self) -> &Path {
        &self.models_dir
    }

    fn final_path(&self, entry: &ModelEntry) -> PathBuf {
        entry.local_path(&self.models_dir)
    }

    fn partial_path(&self, entry: &ModelEntry) -> PathBuf {
        self.models_dir
            .join(format!("{}.{PARTIAL_SUFFIX}", entry.filename))
    }

    /// Inspects one model without touching the network.
    pub fn status(&self, entry: &ModelEntry) -> ModelStatus {
        let final_path = self.final_path(entry);
        if let Ok(metadata) = std::fs::metadata(&final_path) {
            let actual = metadata.len();
            let expected = entry.integrity.size_bytes;
            if actual != expected {
                return ModelStatus::Corrupted {
                    reason: format!("expected {expected} bytes on disk, found {actual}"),
                };
            }
            return ModelStatus::Ready { path: final_path };
        }
        match std::fs::metadata(self.partial_path(entry)) {
            Ok(metadata) => ModelStatus::Partial {
                bytes_on_disk: metadata.len(),
            },
            Err(_) => ModelStatus::Missing,
        }
    }

    /// Deletes a model so the next fetch starts clean.
    pub fn remove(&self, entry: &ModelEntry) -> Result<()> {
        for path in [self.final_path(entry), self.partial_path(entry)] {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).context(format!("removing {}", path.display())),
            }
        }
        Ok(())
    }

    /// Downloads and verifies a model, resuming a partial file if one exists.
    ///
    /// Returns the verified path. Cancelling leaves the partial in place;
    /// a failed verification does not.
    pub async fn fetch<F>(
        &self,
        entry: &ModelEntry,
        cancel: CancellationToken,
        mut on_progress: F,
    ) -> Result<PathBuf>
    where
        F: FnMut(Progress) + Send,
    {
        if let ModelStatus::Ready { path } = self.status(entry) {
            return Ok(path);
        }
        std::fs::create_dir_all(&self.models_dir)
            .with_context(|| format!("creating {}", self.models_dir.display()))?;

        let partial_path = self.partial_path(entry);
        let existing = std::fs::metadata(&partial_path)
            .map(|m| m.len())
            .unwrap_or(0);
        // A partial larger than the whole file is not a resume point.
        let resume_from = if existing >= entry.integrity.size_bytes {
            let _ = std::fs::remove_file(&partial_path);
            0
        } else {
            existing
        };

        let url = registry::download_url(entry, &self.base_url);
        let mut request = self.client.get(&url);
        if resume_from > 0 {
            debug!(
                model = entry.key,
                resume_from, "resuming a partial download"
            );
            request = request.header(reqwest::header::RANGE, format!("bytes={resume_from}-"));
        }

        let response = request
            .send()
            .await
            .map_err(|error| describe_network_error(&error))
            .with_context(|| format!("requesting {url}"))?;

        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("{url} returned HTTP {status}");
        }
        // 206 means the server honored the range; 200 means it ignored it and
        // is sending the whole file, so the partial must be discarded.
        let appending = resume_from > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT;
        let mut downloaded = if appending { resume_from } else { 0 };

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(!appending)
            .open(&partial_path)
            .await
            .with_context(|| format!("opening {}", partial_path.display()))?;
        if appending {
            file.seek(SeekFrom::Start(resume_from)).await?;
        }

        let mut stream = response.bytes_stream();
        loop {
            let next = tokio::select! {
                _ = cancel.cancelled() => {
                    file.flush().await.ok();
                    info!(model = entry.key, "download cancelled; the partial file is kept for resume");
                    return Err(anyhow!("cancelled"));
                }
                next = tokio::time::timeout(CHUNK_STALL_TIMEOUT, stream.next()) => next,
            };

            let chunk = match next {
                Ok(Some(Ok(chunk))) => chunk,
                Ok(Some(Err(error))) => {
                    file.flush().await.ok();
                    return Err(describe_network_error(&error));
                }
                Ok(None) => break,
                Err(_elapsed) => {
                    file.flush().await.ok();
                    anyhow::bail!(
                        "the download stalled for {}s — the connection went quiet without \
                         closing. The partial file is kept, so retrying resumes.",
                        CHUNK_STALL_TIMEOUT.as_secs()
                    );
                }
            };

            file.write_all(&chunk).await?;
            downloaded += chunk.len() as u64;
            on_progress(Progress {
                downloaded_bytes: downloaded,
                total_bytes: entry.integrity.size_bytes,
            });
        }
        file.flush().await?;
        file.sync_all().await?;
        drop(file);

        // Verify before promoting: a file only reaches its real name once its
        // bytes are known-good, so a half-written model is never loadable.
        if let Err(error) = verify(&partial_path, &entry.integrity).await {
            let _ = std::fs::remove_file(&partial_path);
            return Err(error).with_context(|| {
                format!(
                    "{} failed verification and was discarded; run the fetch again",
                    entry.key
                )
            });
        }

        let final_path = self.final_path(entry);
        std::fs::rename(&partial_path, &final_path).with_context(|| {
            format!(
                "promoting {} to {}",
                partial_path.display(),
                final_path.display()
            )
        })?;
        info!(model = entry.key, path = %final_path.display(), "model ready");
        Ok(final_path)
    }

    /// Fetches everything required that is not already present.
    pub async fn fetch_required<F>(
        &self,
        cancel: CancellationToken,
        mut on_progress: F,
    ) -> Result<Vec<PathBuf>>
    where
        F: FnMut(&ModelEntry, Progress) + Send,
    {
        let mut paths = Vec::new();
        for entry in registry::required() {
            let path = self
                .fetch(entry, cancel.clone(), |progress| {
                    on_progress(entry, progress)
                })
                .await?;
            paths.push(path);
        }
        Ok(paths)
    }
}

/// Checks a downloaded file against everything the entry pins.
async fn verify(path: &Path, integrity: &Integrity) -> Result<()> {
    let path = path.to_path_buf();
    let integrity = *integrity;
    tokio::task::spawn_blocking(move || verify_blocking(&path, &integrity))
        .await
        .context("the verification task panicked")?
}

fn verify_blocking(path: &Path, integrity: &Integrity) -> Result<()> {
    use std::io::Read;

    let metadata = std::fs::metadata(path)?;
    if metadata.len() != integrity.size_bytes {
        anyhow::bail!(
            "size mismatch: expected {} bytes, got {}",
            integrity.size_bytes,
            metadata.len()
        );
    }

    // Cheap structural check before the expensive hash: a GGML/GGUF model
    // that does not start with its magic is an error page or a truncation,
    // and saying so is more useful than "checksum mismatch".
    let mut file = std::fs::File::open(path)?;
    let mut magic = [0u8; 4];
    if file.read_exact(&mut magic).is_ok() {
        let looks_like_ggml = &magic == b"ggml" || &magic == b"lmgg" || &magic == b"GGUF";
        let is_model = path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("bin"));
        if is_model && !looks_like_ggml {
            anyhow::bail!(
                "this does not look like a model file (it starts with {magic:?}) — \
                 the mirror may have returned an error page"
            );
        }
    }

    let mut file = std::fs::File::open(path)?;
    let mut sha = sha2::Sha256::new();
    let mut crc = crc32fast::Hasher::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if integrity.sha256.is_some() {
            sha.update(&buffer[..read]);
        }
        if integrity.crc32.is_some() {
            crc.update(&buffer[..read]);
        }
    }

    if let Some(expected) = integrity.sha256 {
        let actual = hex(&sha.finalize());
        if !actual.eq_ignore_ascii_case(expected) {
            anyhow::bail!("sha256 mismatch: expected {expected}, got {actual}");
        }
    }
    if let Some(expected) = integrity.crc32 {
        let actual = crc.finalize();
        if actual != expected {
            anyhow::bail!("crc32 mismatch: expected {expected}, got {actual}");
        }
    }
    if integrity.sha256.is_none() && integrity.crc32.is_none() {
        warn!("this model pins no checksum; its bytes were not verified");
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Turns a transport failure into something an Operator can act on.
fn describe_network_error(error: &reqwest::Error) -> anyhow::Error {
    if error.is_timeout() {
        anyhow!("the connection timed out — check your internet and try again")
    } else if error.is_connect() {
        anyhow!(
            "could not reach the model host — check your internet, or set a mirror with {}",
            registry::BASE_URL_ENV
        )
    } else if error.is_body() || error.is_decode() {
        anyhow!("the connection dropped mid-download — retrying will resume")
    } else {
        anyhow!("{error}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_entry(size: u64, crc: u32) -> ModelEntry {
        ModelEntry {
            key: "test-model",
            display_name: "Test",
            filename: "test-model.bin",
            remote_path: "test-model.bin",
            integrity: Integrity {
                size_bytes: size,
                sha256: None,
                crc32: Some(crc),
            },
            purpose: registry::ModelPurpose::Transcription,
            required: true,
            provenance: registry::Provenance {
                license: "MIT",
                source: "https://example.invalid/fixture",
            },
            driving: None,
        }
    }

    fn ggml_payload(len: usize) -> Vec<u8> {
        let mut bytes = b"ggml".to_vec();
        bytes.extend((0..len - 4).map(|index| (index % 251) as u8));
        bytes
    }

    #[test]
    fn status_reports_missing_partial_and_ready() {
        let dir = tempfile::tempdir().expect("tempdir");
        let payload = ggml_payload(64);
        let entry = test_entry(64, crc32fast::hash(&payload));
        let downloader =
            Downloader::with_base_url(dir.path().to_path_buf(), "http://unused".into()).unwrap();

        assert_eq!(downloader.status(&entry), ModelStatus::Missing);

        std::fs::write(dir.path().join("test-model.bin.partial"), &payload[..10]).unwrap();
        assert_eq!(
            downloader.status(&entry),
            ModelStatus::Partial { bytes_on_disk: 10 }
        );

        std::fs::write(dir.path().join("test-model.bin"), &payload).unwrap();
        assert!(downloader.status(&entry).is_ready());
    }

    #[test]
    fn a_wrong_sized_file_is_corrupted_not_ready() {
        let dir = tempfile::tempdir().expect("tempdir");
        let entry = test_entry(64, 0);
        let downloader =
            Downloader::with_base_url(dir.path().to_path_buf(), "http://unused".into()).unwrap();
        std::fs::write(dir.path().join("test-model.bin"), b"too short").unwrap();

        match downloader.status(&entry) {
            ModelStatus::Corrupted { reason } => assert!(reason.contains("64")),
            other => panic!("expected Corrupted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn verification_rejects_the_wrong_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("model.bin");
        let payload = ggml_payload(128);
        std::fs::write(&path, &payload).unwrap();

        let good = Integrity {
            size_bytes: 128,
            sha256: None,
            crc32: Some(crc32fast::hash(&payload)),
        };
        verify(&path, &good).await.expect("the real bytes verify");

        let bad = Integrity {
            size_bytes: 128,
            sha256: None,
            crc32: Some(1),
        };
        let error = verify(&path, &bad).await.expect_err("a bad crc must fail");
        assert!(error.to_string().contains("crc32 mismatch"), "{error}");
    }

    #[tokio::test]
    async fn a_html_error_page_is_named_as_such_not_as_a_checksum_failure() {
        // The failure a bad mirror actually produces: 200 OK with an error
        // page in the body. "checksum mismatch" would send the Operator
        // looking in the wrong place.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("model.bin");
        let payload = b"<!DOCTYPE html><html>404</html>".to_vec();
        std::fs::write(&path, &payload).unwrap();

        let integrity = Integrity {
            size_bytes: payload.len() as u64,
            sha256: None,
            crc32: Some(crc32fast::hash(&payload)),
        };
        let error = verify(&path, &integrity)
            .await
            .expect_err("an error page must not pass as a model");
        assert!(
            error
                .to_string()
                .contains("does not look like a model file"),
            "{error}"
        );
    }

    #[test]
    fn progress_reports_a_sane_fraction() {
        let progress = Progress {
            downloaded_bytes: 50,
            total_bytes: 200,
        };
        assert!((progress.fraction() - 0.25).abs() < f64::EPSILON);

        let unknown = Progress {
            downloaded_bytes: 10,
            total_bytes: 0,
        };
        assert_eq!(
            unknown.fraction(),
            0.0,
            "an unknown total is not a divide by zero"
        );

        let over = Progress {
            downloaded_bytes: 300,
            total_bytes: 200,
        };
        assert_eq!(over.fraction(), 1.0, "progress never exceeds 1");
    }
}
