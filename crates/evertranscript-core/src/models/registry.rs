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

/// How this model's vectors are labelled wherever they are stored.
///
/// **Deliberately not the registry `key`.** The key is a download address
/// and is lower-case by convention; the stored identity is whatever was
/// written into existing Voiceprints and cannot be changed without
/// retagging every one of them. For the embedding model those two differ in
/// exactly one character — `-lm` against `-LM` — so a tidy-minded change
/// from one to the other would orphan every Voiceprint on every installed
/// copy, and each returning speaker would come back as a stranger. That is
/// what `the_stored_identity_is_not_the_download_key` exists to prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceprintId {
    /// The `model` column written on every exemplar.
    pub model: &'static str,
    /// The `model_version` column. Bumped when the *front end* changes, not
    /// only the weights: a version 1 and a version 2 vector of the same
    /// audio agree at cosine 0.36 and must never be compared.
    pub version: &'static str,
    /// What the graph eats. Part of the identity rather than a loader
    /// argument because a front end mismatch does not fail — it returns a
    /// plausible vector of the wrong thing (DECISIONS Q115), so the one
    /// place that names the model is the one place that names its input.
    pub frontend: Frontend,
}

/// How an embedding graph takes its audio.
///
/// Two of the candidates measured for this product differ here and nowhere
/// visible: WeSpeaker wants Kaldi filterbank features computed on the Rust
/// side, ReDimNet2 wants raw 16 kHz samples and carries its own mel. The
/// bake-off that first chose between them ran both through one front end,
/// which is the cautionary tale `VoiceprintId::frontend` exists for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frontend {
    /// Kaldi fbank computed here, fed as `input_features`.
    Fbank,
    /// Raw 16 kHz waveform, fed as `waveform`; the graph owns the mel.
    Waveform,
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
    /// How this model's vectors are labelled in the store. `None` for every
    /// model that does not produce stored vectors.
    pub voiceprint: Option<VoiceprintId>,
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

    /// The stored identity, for a model that has one.
    ///
    /// `const` so the one place that stamps vectors can take it from here
    /// rather than repeating the strings, which is the whole point: the
    /// registry entry is the source, and a literal somewhere else is how the
    /// two drift apart.
    pub const fn voiceprint(&self) -> VoiceprintId {
        match self.voiceprint {
            Some(identity) => identity,
            None => panic!("this model does not produce stored vectors"),
        }
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
    voiceprint: None,
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
    voiceprint: None,
};

/// The embedding that makes a Voiceprint: ReDimNet2-B3, trained on
/// VoxBlink2 + VoxCeleb2 with large-margin fine-tuning (PalabraAI/redimnet2
/// v1.0.0, MIT). Adopted 2026-09-17 on the measured record (DECISIONS Q140,
/// Q143): 26.01% against WeSpeaker's 29.13% DER on AMI dev, 24.62% against
/// 28.64% held-out, all of the held-out gap confusion.
///
/// The graph takes raw 16 kHz waveform and owns its mel front end, so the
/// entry says [`Frontend::Waveform`] and `live` feeds it samples; WeSpeaker
/// wanted Kaldi fbank computed here. The file is our own export —
/// `scripts/export-redimnet2.py`, checked at cosine 1.0 against PyTorch —
/// published unchanged to a Hugging Face org this product controls, the
/// same host as every other Provisioned Model (ADR-0034).
///
/// Same `filename` as the model it replaces, on purpose: an installed copy
/// still holding WeSpeaker's 26,535,549 bytes under that name reads as
/// `Corrupted` on size and is fetched afresh. Its Voiceprints are 256 wide
/// and stamped with the old identity. `resolve` never sees those — but the
/// next ordinary Diarization does: `stale_exemplars` finds every row from
/// another space, `runner::rebuild` re-embeds each from the window WeSpeaker
/// cut, and `adopt_rebuilt` files the result under this identity (Q227).
/// That is the lazy path ADR-0037 rejected in favour of the wipe and re-run
/// still pending in `store::schema`; until those are registered, it runs.
pub const DIARIZE_EMBEDDING: ModelEntry = ModelEntry {
    key: "redimnet2-b3-vox2-lm",
    display_name: "ReDimNet2-B3 (VoxBlink2 + VoxCeleb2, LM)",
    filename: "diarize-embedding.onnx",
    remote_path: "soulmachine/evertranscript-redimnet2-b3-vox2-lm/resolve/main/redimnet2-b3-vox2-lm.onnx",
    integrity: Integrity {
        size_bytes: 18_045_013,
        sha256: Some("dcecdce7d52bbd4739b24d0874359ec564d43f4b3a392f0104f505593b566d41"),
        crc32: None,
    },
    purpose: ModelPurpose::Diarization,
    required: true,
    provenance: Provenance {
        license: "MIT",
        source: "https://huggingface.co/soulmachine/evertranscript-redimnet2-b3-vox2-lm",
    },
    driving: None,
    voiceprint: Some(VoiceprintId {
        model: "redimnet2-b3",
        version: "1",
        frontend: Frontend::Waveform,
    }),
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
    key: "qwen3-4b-ud-q4_k_xl",
    display_name: "Qwen3 4B (UD-Q4_K_XL)",
    filename: "summary-qwen3-4b-ud-q4_k_xl.gguf",
    remote_path: "unsloth/Qwen3-4B-GGUF/resolve/main/Qwen3-4B-UD-Q4_K_XL.gguf",
    integrity: Integrity {
        // Verified against the publisher's LFS metadata, which is also what
        // the `x-linked-etag` header carries. **Not the CDN's `etag`** —
        // that is a Xet content hash and will never match. The plain
        // `Q4_K_M` build of the same model also starts `f6`, so a prefix
        // comparison picks the wrong file.
        size_bytes: 2_546_341_152,
        sha256: Some("f6e3fb6c2cdc869d16e66c719e94f2c02095d195967230e759a2d77fe814c71f"),
        crc32: None,
    },
    purpose: ModelPurpose::Summary,
    // **Provisioned**: fetched by default, because a Summary feature that is
    // there when reached is the point of choosing a model this size. Summary
    // is still not an Anchor — it keeps its Knob — and the glossary
    // distinguishes the two.
    required: true,
    provenance: Provenance {
        license: "Apache-2.0",
        source: "https://huggingface.co/unsloth/Qwen3-4B-GGUF",
    },
    driving: Some(Driving {
        // Qwen3 is trained on ChatML and ships the template in the GGUF.
        // Reading it from the model rather than naming a format is what keeps
        // this right for whatever is registered next.
        framing: Framing::EmbeddedChatTemplate,
        // The publisher's non-thinking settings, and greedy is not among
        // them: the card says "DO NOT use greedy decoding", whose failure
        // mode is the degenerate repetition this sidecar already needed a
        // penalty to survive.
        sampling: Sampling::Nucleus {
            temperature: 0.7,
            top_p: 0.8,
            top_k: 20,
            min_p: 0.0,
        },
        // The hard switch is a template variable this API cannot reach, so
        // the soft switch it is — documented by the publisher for exactly
        // this runtime. A Summary is a document, not a reasoning trace, and
        // `scrub` would discard the thinking after we had paid to generate it.
        suppress_reasoning: Some("/no_think"),
        // The model declares far more, but the KV cache costs most exactly
        // when a Summary competes with a recording. Three times the previous
        // budget, well inside what the model can take.
        context_tokens: 16_384,
        single_pass_tokens: 12_000,
    }),
    voiceprint: None,
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

    /// The one-character difference is the whole test.
    ///
    /// `key` is an address and `voiceprint().model` is a label on rows that
    /// already exist. Someone will one day notice they differ only in case
    /// and reach for the tidy fix; this is what stops them. Changing either
    /// side of this assertion means every installed copy's Voiceprints have
    /// to be retagged in the same change.
    #[test]
    fn the_stored_identity_is_not_the_download_key() {
        let stored = DIARIZE_EMBEDDING.voiceprint();
        assert_eq!(
            (stored.model, stored.version),
            ("redimnet2-b3", "1"),
            "this is what is written on every Voiceprint from now on; \
             the earlier WeSpeaker stamp is what the model-change wipe clears"
        );
        assert_ne!(
            DIARIZE_EMBEDDING.key, stored.model,
            "they differ only in case, which is exactly why one cannot stand in for the other"
        );
    }

    /// A model that stores no vectors has nothing to say about Voiceprints,
    /// and saying it anyway would be a second place for the identity to live.
    #[test]
    fn only_the_embedding_model_carries_a_voiceprint_identity() {
        let carrying: Vec<&str> = ALL
            .iter()
            .filter(|entry| entry.voiceprint.is_some())
            .map(|entry| entry.key)
            .collect();
        assert_eq!(carrying, vec![DIARIZE_EMBEDDING.key]);
    }

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
    fn the_registered_summary_model_is_described_as_its_publisher_documents() {
        let driving = SUMMARY_DEFAULT
            .driving
            .expect("the Summary model is prompted");
        assert_eq!(driving.framing, Framing::EmbeddedChatTemplate);
        assert_eq!(
            driving.sampling,
            Sampling::Nucleus {
                temperature: 0.7,
                top_p: 0.8,
                top_k: 20,
                min_p: 0.0,
            }
        );
        assert_eq!(driving.suppress_reasoning, Some("/no_think"));
        assert_eq!(driving.context_tokens, 16_384);
        assert_eq!(driving.single_pass_tokens, 12_000);
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
