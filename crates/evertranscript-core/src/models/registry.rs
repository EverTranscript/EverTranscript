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

/// How a model wants its prompt shaped.
///
/// A property of the model rather than of the product: an instruct model
/// trained on ChatML answers a ChatML prompt better than a flat one, and the
/// next model may want neither. Hardcoding one framing in the sidecar is what
/// makes swapping a model a code change instead of a data change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    /// System, then user, then a bare `Summary:` cue. What every model here
    /// was driven with before framing became a property.
    Plain,
    /// The chat template embedded in the GGUF, applied to system and user as
    /// separate turns. Applying one has no fallback, so a model without a
    /// template must say `Plain`.
    EmbeddedChatTemplate,
}

/// How a model wants to be sampled.
///
/// **Greedy is a choice, not an absence of one**, and some models' own
/// documentation forbids it — degenerate repetition is the failure it invites,
/// which is why the sidecar needed a repetition penalty in the first place.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sampling {
    /// Always the highest-probability token.
    Greedy,
    /// A distribution, narrowed by the model's published settings.
    Nucleus {
        temperature: f32,
        top_p: f32,
        top_k: i32,
        min_p: f32,
    },
}

/// Everything about driving a model that is true of the model rather than of
/// the product.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Driving {
    pub framing: Framing,
    pub sampling: Sampling,
    /// Text appended to the system turn to stop a reasoning model thinking
    /// aloud. **Not part of the Operator's editable prompt**: an Operator who
    /// rewrote their prompt would silently re-enable reasoning, and pay for
    /// tokens that are discarded before they ever see them.
    pub suppress_reasoning: Option<&'static str>,
    /// Context to allocate, and the size below which a meeting is summarized
    /// in one pass. Both were constants sized for a 0.5B.
    pub context_tokens: u32,
    pub single_pass_tokens: usize,
}

/// Where an artifact came from and under what terms.
///
/// This repository keeps a careful ledger for every *file* it ported
/// (`PORTS.md`), and said nothing about the half-gigabyte artifacts it
/// downloads. Recorded per entry rather than in that ledger because a model
/// has no attribution header and no upstream revision — the discipline
/// PORTS.md enforces does not apply to it, and diluting that ledger would
/// cost more than it gains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Provenance {
    /// SPDX identifier, e.g. `Apache-2.0`.
    pub license: &'static str,
    /// Where it is published, for a human following it up.
    pub source: &'static str,
}

/// One required artifact.
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
    /// Licence and source. Every entry carries one.
    pub provenance: Provenance,
    /// How a generative model wants to be driven. `None` for models that are
    /// not prompted at all — the ONNX pair, and whisper.
    pub driving: Option<Driving>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelPurpose {
    /// Live transcription — the Anchor, permanently local (ADR-0002).
    Transcription,
    /// Echo cancellation on the mic channel (ADR-0029).
    ///
    /// No entry carries this: the AEC shipped as an NLMS adaptive filter
    /// rather than the ADR's ONNX pair, because both legs are stamped on one
    /// capture clock and are therefore already aligned — which is the part
    /// NLMS is good at. Kept because the ADR names the purpose.
    EchoCancellation,
    /// Diarization: who spoke (ADR-0008, ADR-0029 as amended).
    Diarization,
    /// Local Summary, through the bundled sidecar (ADR-0031).
    Summary,
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
    provenance: Provenance {
        license: "MIT",
        source: "https://huggingface.co/ggerganov/whisper.cpp",
    },
    // Not prompted: whisper is handed audio, not a conversation.
    driving: None,
};

/// Speaker segmentation: where speech is, and where two voices overlap.
///
/// Sizes and checksums are read off the downloaded artifacts, not copied
/// from a listing. The signature was read the same way rather than assumed:
/// `input_values [batch, channels, samples]` of raw waveform, and `logits
/// [batch, frames, 7]` — the powerset over three speakers the catalog
/// describes.
pub const DIARIZE_SEGMENTATION: ModelEntry = ModelEntry {
    key: "pyannote-segmentation-3.0",
    display_name: "pyannote segmentation 3.0",
    filename: "diarize-segmentation.onnx",
    remote_path: "onnx-community/pyannote-segmentation-3.0/resolve/main/onnx/model.onnx",
    integrity: Integrity {
        size_bytes: 5_986_908,
        sha256: Some("057ee564753071c0b09b5b611648b50ac188d50846bff5f01e9f7bbf1591ea25"),
        crc32: None,
    },
    purpose: ModelPurpose::Diarization,
    required: true,
    provenance: Provenance {
        license: "MIT",
        source: "https://huggingface.co/onnx-community/pyannote-segmentation-3.0",
    },
    driving: None,
};

/// Speaker embedding: the vector a Voiceprint is made of.
///
/// `input_features [B, T, 80]` — the 80-mel filterbank `diarize::fbank`
/// computes — and `last_hidden_state [B, 256]`.
pub const DIARIZE_EMBEDDING: ModelEntry = ModelEntry {
    key: "wespeaker-voxceleb-resnet34-lm",
    display_name: "WeSpeaker VoxCeleb ResNet34-LM",
    filename: "diarize-embedding.onnx",
    remote_path: "onnx-community/wespeaker-voxceleb-resnet34-LM/resolve/main/onnx/model.onnx",
    integrity: Integrity {
        size_bytes: 26_535_549,
        sha256: Some("3955447b0499dc9e0a4541a895df08b03c69098eba4e56c02b5603e9f7f4fcbb"),
        crc32: None,
    },
    purpose: ModelPurpose::Diarization,
    required: true,
    provenance: Provenance {
        license: "Apache-2.0",
        source: "https://huggingface.co/onnx-community/wespeaker-voxceleb-resnet34-LM",
    },
    driving: None,
};

/// The local Summary model (ADR-0031: "its small instruct model downloads
/// during onboarding when the Operator picks Local").
///
/// **This is the model that was verified, not the model that should ship.**
/// 0.5B is small enough to prove the sidecar end to end on a laptop and is
/// demonstrably too weak for the job: on a two-line transcript it produced a
/// correct summary and then attributed one person's commitment to the other.
/// A larger default belongs to the close-out's quality measurement rather
/// than to anyone's reputation — which is the whole reason M4 owes a number.
/// Size and checksum read off the downloaded artifact.
pub const SUMMARY_DEFAULT: ModelEntry = ModelEntry {
    key: "qwen2.5-0.5b-instruct-q4_k_m",
    display_name: "Qwen2.5 0.5B Instruct (q4_K_M)",
    filename: "summary-qwen2.5-0.5b-instruct-q4_k_m.gguf",
    remote_path: "Qwen/Qwen2.5-0.5B-Instruct-GGUF/resolve/main/qwen2.5-0.5b-instruct-q4_k_m.gguf",
    integrity: Integrity {
        size_bytes: 491_400_032,
        sha256: Some("74a4da8c9fdbcd15bd1f6d01d621410d31c6fc00986f5eb687824e7b93d7a9db"),
        crc32: None,
    },
    purpose: ModelPurpose::Summary,
    // Not required: Summary is not an Anchor (ADR-0002), and a machine that
    // never generates one is a working installation. Marking it required
    // would make a fresh install refuse to record until half a gigabyte had
    // downloaded, for a feature the Operator may not have chosen.
    required: false,
    provenance: Provenance {
        license: "Apache-2.0",
        source: "https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF",
    },
    // **Exactly how the sidecar drove this model before it was described**,
    // so introducing the seam changes no output. Greedy and plain framing are
    // recorded as the choices they always were rather than adopted here.
    driving: Some(Driving {
        framing: Framing::Plain,
        sampling: Sampling::Greedy,
        suppress_reasoning: None,
        context_tokens: 8_192,
        single_pass_tokens: 4_000,
    }),
};

/// Every artifact this build knows how to fetch.
pub const ALL: &[ModelEntry] = &[
    WHISPER_DEFAULT,
    DIARIZE_SEGMENTATION,
    DIARIZE_EMBEDDING,
    SUMMARY_DEFAULT,
];

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

    #[test]
    fn every_model_records_where_it_came_from_and_under_what_terms() {
        // This repository keeps a careful ledger for every file it ported and
        // said nothing about the artifacts it downloads, which is an odd
        // silence for a public Apache-2.0 project.
        for entry in ALL {
            assert!(
                !entry.provenance.license.is_empty(),
                "{} has no licence",
                entry.key
            );
            assert!(
                entry.provenance.source.starts_with("https://"),
                "{} has no followable source, got {:?}",
                entry.key,
                entry.provenance.source
            );
        }
    }

    #[test]
    fn only_the_prompted_model_says_how_to_drive_it() {
        // Whisper is handed audio and the ONNX pair are handed tensors; a
        // sampling temperature would be meaningless on any of them.
        for entry in ALL {
            match entry.purpose {
                ModelPurpose::Summary => assert!(
                    entry.driving.is_some(),
                    "{} is prompted and must say how",
                    entry.key
                ),
                _ => assert!(
                    entry.driving.is_none(),
                    "{} is not prompted and should not describe driving",
                    entry.key
                ),
            }
        }
    }

    #[test]
    fn the_registered_summary_model_is_described_as_it_has_always_been_driven() {
        // The guard on this ticket: introducing the seam must change no
        // output. When the model changes, this test changes with it — and
        // that is the point, because then the change is visible in a diff
        // rather than absorbed silently.
        let driving = SUMMARY_DEFAULT
            .driving
            .expect("the Summary model is prompted");
        assert_eq!(driving.framing, Framing::Plain);
        assert_eq!(driving.sampling, Sampling::Greedy);
        assert_eq!(driving.suppress_reasoning, None);
        assert_eq!(driving.context_tokens, 8_192);
        assert_eq!(driving.single_pass_tokens, 4_000);
    }

    #[test]
    fn a_model_that_wants_a_chat_template_can_say_so() {
        // The shape exists before the model that needs it, so adopting one is
        // a data change rather than a code change.
        let driving = Driving {
            framing: Framing::EmbeddedChatTemplate,
            sampling: Sampling::Nucleus {
                temperature: 0.7,
                top_p: 0.8,
                top_k: 20,
                min_p: 0.0,
            },
            suppress_reasoning: Some("/no_think"),
            context_tokens: 16_384,
            single_pass_tokens: 12_000,
        };
        assert_ne!(driving.framing, Framing::Plain);
        assert_ne!(driving.sampling, Sampling::Greedy);
    }
}
