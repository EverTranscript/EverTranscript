//! What the Core needs on disk before it can work, and where to get it.
//!
//! Models are not part of the History folder: they are re-downloadable, so
//! they live in Application Support and never travel with the record
//! (ADR-0035). Every entry pins an exact size and a checksum, because a
//! truncated or corrupted model fails in ways that look like bad
//! transcription rather than like a bad download.

use std::path::PathBuf;

/// How a downloaded file is verified.
///
/// SHA-256 is what we want everywhere. The Whisper entry currently pins the
/// CRC32 that anarlog's shipped registry verified, because pinning a SHA-256
/// means downloading and hashing the artifact first. Both are checked when
/// both are present.
///
/// **Release blocker:** every entry must carry `sha256` before v1 ships.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Integrity {
    pub size_bytes: u64,
    pub sha256: Option<&'static str>,
    pub crc32: Option<u32>,
}

impl Integrity {
    /// True when this entry meets the release bar (a strong checksum).
    pub fn is_strongly_pinned(&self) -> bool {
        self.sha256.is_some()
    }
}

/// One required artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelEntry {
    /// Stable key used by the protocol and the CLI.
    pub key: &'static str,
    /// Human name for the UI.
    pub display_name: &'static str,
    /// Filename on disk, and the path suffix on every mirror.
    pub filename: &'static str,
    /// Path relative to a mirror root, e.g. `ggerganov/whisper.cpp/...`.
    pub remote_path: &'static str,
    pub integrity: Integrity,
    pub purpose: ModelPurpose,
    /// False for artifacts a feature can run without.
    pub required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelPurpose {
    /// Live transcription — the Anchor, permanently local (ADR-0002).
    Transcription,
    /// Echo cancellation on the mic channel (ADR-0029).
    EchoCancellation,
}

impl ModelEntry {
    pub fn local_path(&self, models_dir: &std::path::Path) -> PathBuf {
        models_dir.join(self.filename)
    }
}

/// The shipped default: best multilingual and Chinese quality whisper.cpp
/// offers at real-time speed on Apple Silicon (PRD; Settings can select a
/// smaller one).
pub const WHISPER_DEFAULT: ModelEntry = ModelEntry {
    key: "whisper-large-v3-turbo-q8_0",
    display_name: "Whisper large-v3-turbo (q8_0)",
    filename: "ggml-large-v3-turbo-q8_0.bin",
    remote_path: "ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q8_0.bin",
    integrity: Integrity {
        size_bytes: 874_188_075,
        sha256: None,
        crc32: Some(3_055_274_469),
    },
    purpose: ModelPurpose::Transcription,
    required: true,
};

/// Every artifact this build knows how to fetch.
pub const ALL: &[ModelEntry] = &[WHISPER_DEFAULT];

pub fn find(key: &str) -> Option<&'static ModelEntry> {
    ALL.iter().find(|entry| entry.key == key)
}

pub fn required() -> impl Iterator<Item = &'static ModelEntry> {
    ALL.iter().filter(|entry| entry.required)
}

/// Where models are fetched from.
///
/// Hugging Face is the default; the mirror is Operator-configurable because
/// Hugging Face is unreliable-to-blocked in China, and the Watchlist already
/// ships VooV. Pinned checksums are what make an arbitrary mirror safe: any
/// mirror, same verified bytes.
pub const DEFAULT_BASE_URL: &str = "https://huggingface.co";

/// Environment override, mostly for tests and for Operators behind a proxy.
pub const BASE_URL_ENV: &str = "EVERTRANSCRIPT_MODEL_BASE_URL";

pub fn base_url() -> String {
    std::env::var(BASE_URL_ENV).unwrap_or_else(|_| DEFAULT_BASE_URL.to_string())
}

pub fn download_url(entry: &ModelEntry, base_url: &str) -> String {
    format!("{}/{}", base_url.trim_end_matches('/'), entry.remote_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_entry_has_a_unique_key_and_filename() {
        for (index, entry) in ALL.iter().enumerate() {
            for other in &ALL[index + 1..] {
                assert_ne!(entry.key, other.key, "duplicate key");
                assert_ne!(entry.filename, other.filename, "duplicate filename");
            }
        }
    }

    #[test]
    fn every_entry_can_be_verified_somehow() {
        for entry in ALL {
            assert!(
                entry.integrity.sha256.is_some() || entry.integrity.crc32.is_some(),
                "{} must pin a checksum: an unverified model is how a truncated \
                 download turns into bad transcription",
                entry.key
            );
            assert!(
                entry.integrity.size_bytes > 0,
                "{} must pin a size",
                entry.key
            );
        }
    }

    #[test]
    fn the_mirror_url_composes_correctly() {
        assert_eq!(
            download_url(&WHISPER_DEFAULT, "https://example.test/"),
            "https://example.test/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q8_0.bin"
        );
    }

    /// Not a failure: a standing reminder that shipping needs SHA-256.
    #[test]
    fn report_entries_still_missing_a_strong_checksum() {
        let weak: Vec<&str> = ALL
            .iter()
            .filter(|entry| !entry.integrity.is_strongly_pinned())
            .map(|entry| entry.key)
            .collect();
        if !weak.is_empty() {
            eprintln!("note: these models are release-blocked until a SHA-256 is pinned: {weak:?}");
        }
    }
}
