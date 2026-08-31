//! The OpenAI-compatible Backend — the only path in this product that may
//! carry meeting content over the network.
//!
//! One client serves every destination, because ADR-0031 already assumes the
//! OpenAI-compatible shape: a cloud preset, a custom endpoint, Ollama and LM
//! Studio differ only by base URL and whether a key is needed. That is worth
//! more than an abstraction per provider — **this is the largest exfiltration
//! surface in the product, and it should be readable in one sitting.**
//!
//! What the request carries is deliberately minimal: the system prompt and
//! the user message, and nothing about the machine. No identifiers, no
//! version string, no meeting metadata the Summary does not need. Everything
//! sent is something the Operator could have pasted themselves.

use std::time::Duration;

use super::Backend;
use super::BackendError;
use super::BackendIdentity;
use super::Cancel;
use super::Request;

/// How long to wait before calling it a timeout.
///
/// Generous, because summarizing ninety minutes on a slow model is slow, and
/// a fallback triggered by impatience is a fallback the Operator did not
/// need.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

/// What a provider does with what you send it.
///
/// **Labels are information, never gates** (ADR-0010). The product cannot
/// verify provider-side retention, so refusing a provider on a label would
/// be false hardness dressed as a guarantee — and it would block the
/// Operator's own explicit choice, which this product does not do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataHandling {
    /// Whether inputs are used for training, per the provider's API terms.
    pub trains_on_inputs: bool,
    /// How long inputs are kept, as the provider states it.
    pub retention: &'static str,
    /// Whether zero-data-retention is available on request.
    pub zero_retention_available: bool,
    /// When a human last read the provider's terms and wrote this down.
    ///
    /// Shown to the Operator. An unverifiable label is worse than none, and
    /// a label with no date is unverifiable — terms change, and a claim with
    /// no date cannot be known to be stale.
    pub verified_on: &'static str,
}

/// A curated destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    pub id: &'static str,
    pub display_name: &'static str,
    pub base_url: &'static str,
    pub default_model: &'static str,
    /// None for endpoints whose terms are not ours to characterise — the
    /// custom field, and any local runtime.
    pub data_handling: Option<DataHandling>,
}

/// The curated list (ADR-0010: no-training-by-default providers only).
///
/// **The labels here are placeholders and are marked as such by their
/// `verified_on` date.** ADR-0010 requires verification at release time by
/// someone who read the terms; nobody has, and writing plausible values with
/// a plausible date would be exactly the false assurance the ADR forbids.
/// The date says `unverified` so the surface can say so too.
pub const PRESETS: &[Preset] = &[
    Preset {
        id: "openai",
        display_name: "OpenAI",
        base_url: "https://api.openai.com/v1",
        default_model: "gpt-4o-mini",
        data_handling: Some(DataHandling {
            trains_on_inputs: false,
            retention: "see provider terms",
            zero_retention_available: true,
            verified_on: "unverified",
        }),
    },
    Preset {
        id: "anthropic",
        display_name: "Anthropic",
        base_url: "https://api.anthropic.com/v1",
        default_model: "claude-sonnet-4-5",
        data_handling: Some(DataHandling {
            trains_on_inputs: false,
            retention: "see provider terms",
            zero_retention_available: true,
            verified_on: "unverified",
        }),
    },
    // Local runtimes, reached through the same client. Not cloud, no key,
    // and no data-handling label because nothing leaves the machine to be
    // handled.
    Preset {
        id: "ollama",
        display_name: "Ollama",
        base_url: "http://localhost:11434/v1",
        default_model: "qwen2.5:3b",
        data_handling: None,
    },
    Preset {
        id: "lmstudio",
        display_name: "LM Studio",
        base_url: "http://localhost:1234/v1",
        default_model: "local-model",
        data_handling: None,
    },
];

/// What the custom base-URL field is labeled (ADR-0010).
pub const CUSTOM_ENDPOINT_LABEL: &str = "unknown endpoint — your rules";

pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|preset| preset.id == id)
}

/// Whether a base URL points at this machine.
///
/// Used to decide whether a destination is `Cloud` or `LocalRuntime`, which
/// decides whether `leaves_the_machine()` is true — so it decides whether
/// the whole Knob treats a run as an exfiltration. **When in doubt it
/// answers "not local"**: mistaking a remote host for a local one would let
/// the fallback path treat a cloud endpoint as a safe destination.
pub fn is_loopback(base_url: &str) -> bool {
    let authority = base_url
        .split("://")
        .nth(1)
        .unwrap_or(base_url)
        .split('/')
        .next()
        .unwrap_or("")
        // Userinfo is everything before the last `@`, and it can contain
        // anything: `https://localhost@evil.example/` is a request to
        // evil.example. Taking the part *after* it is what stops a hostile
        // URL from reading as loopback.
        .rsplit('@')
        .next()
        .unwrap_or("");

    // A bracketed IPv6 literal keeps its colons, so the port cannot be split
    // off with a naive `split(':')` — that turns `[::1]:8080` into `[`.
    let host = match authority.strip_prefix('[') {
        Some(rest) => rest.split_once(']').map(|(host, _)| host).unwrap_or(rest),
        None => authority.split(':').next().unwrap_or(""),
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

/// An OpenAI-compatible Backend.
pub struct CloudBackend {
    client: reqwest::blocking::Client,
    base_url: String,
    model: String,
    api_key: Option<String>,
    identity: BackendIdentity,
}

impl CloudBackend {
    pub fn new(
        display_name: &str,
        base_url: &str,
        model: &str,
        api_key: Option<String>,
    ) -> Result<Self, BackendError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|error| BackendError::Unavailable(error.to_string()))?;

        // A loopback endpoint is someone else's local runtime, not cloud.
        // The distinction is what the Knob's whole asymmetry rests on.
        let identity = if is_loopback(base_url) {
            BackendIdentity::LocalRuntime {
                name: display_name.to_string(),
                model: model.to_string(),
            }
        } else {
            BackendIdentity::Cloud {
                provider: display_name.to_string(),
                model: model.to_string(),
            }
        };

        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            api_key,
            identity,
        })
    }
}

impl Backend for CloudBackend {
    fn generate(&mut self, request: &Request, cancel: &Cancel) -> Result<String, BackendError> {
        if cancel.is_cancelled() {
            return Err(BackendError::Cancelled);
        }

        // Everything sent, in one place. Two messages and a model name —
        // nothing about the machine, the Operator, or the Meeting beyond the
        // text being summarized.
        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                { "role": "system", "content": request.system },
                { "role": "user", "content": request.user },
            ],
            "stream": false,
        });

        let mut post = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .json(&body);
        if let Some(key) = &self.api_key {
            post = post.bearer_auth(key);
        }

        let response = post.send().map_err(|error| {
            if error.is_timeout() {
                BackendError::TimedOut(error.to_string())
            } else {
                BackendError::Unreachable(error.to_string())
            }
        })?;

        let status = response.status();
        if !status.is_success() {
            // The body may carry a key, an org id, or a prompt echo. Only
            // the status is reported: an error message logged verbatim is a
            // way for secrets to reach a log file.
            return Err(BackendError::Refused(format!("HTTP {status}")));
        }

        let parsed: serde_json::Value = response
            .json()
            .map_err(|error| BackendError::Malformed(error.to_string()))?;
        parsed["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| BackendError::Malformed("no message content in the response".into()))
    }

    fn identity(&self) -> BackendIdentity {
        self.identity.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loopback_endpoint_is_local_not_cloud() {
        // The distinction the Knob's entire asymmetry rests on: an Operator
        // running Ollama has chosen local, and treating it as cloud would
        // make the fallback refuse to use it.
        for local in [
            "http://localhost:11434/v1",
            "http://127.0.0.1:1234/v1",
            "http://[::1]:8080/v1",
            "https://localhost/v1",
        ] {
            assert!(is_loopback(local), "{local} should be local");
        }
    }

    #[test]
    fn anything_not_plainly_loopback_is_treated_as_cloud() {
        // When in doubt, "not local". Mistaking a remote host for a local
        // one would let the fallback treat a cloud endpoint as a safe
        // destination — the exact failure this milestone exists to prevent.
        for remote in [
            "https://api.openai.com/v1",
            // Hostnames that merely contain the word.
            "https://localhost.evil.example/v1",
            "https://notlocalhost/v1",
            // Userinfo pointing somewhere else entirely.
            "https://localhost@evil.example/v1",
            "https://user:pass@127.0.0.1.evil.example/v1",
            // A bracketed literal that is not loopback.
            "http://[2001:db8::1]:8080/v1",
            "",
        ] {
            assert!(!is_loopback(remote), "{remote} should not be local");
        }
    }

    #[test]
    fn a_cloud_backend_reports_that_it_leaves_the_machine() {
        let backend =
            CloudBackend::new("OpenAI", "https://api.openai.com/v1", "gpt", None).expect("builds");
        assert!(backend.identity().leaves_the_machine());
    }

    #[test]
    fn a_local_runtime_reports_that_it_does_not() {
        let backend =
            CloudBackend::new("Ollama", "http://localhost:11434/v1", "qwen", None).expect("builds");
        assert!(!backend.identity().leaves_the_machine());
        assert_eq!(backend.identity().label(), "Ollama (qwen)");
    }

    #[test]
    fn every_curated_preset_that_is_cloud_carries_a_label() {
        // ADR-0010: curated presets carry a data-handling label. A cloud
        // destination offered without one is the thing the ADR forbids.
        for preset in PRESETS {
            if is_loopback(preset.base_url) {
                continue;
            }
            assert!(
                preset.data_handling.is_some(),
                "{} is cloud and has no label",
                preset.id
            );
        }
    }

    #[test]
    fn the_labels_admit_they_are_unverified() {
        // ADR-0010 requires verification at release time by someone who read
        // the terms. Nobody has. Writing plausible values with a plausible
        // date would be exactly the false assurance the ADR forbids, so the
        // date says so and this test keeps it honest until a human fixes it.
        for preset in PRESETS {
            if let Some(handling) = &preset.data_handling {
                assert_eq!(
                    handling.verified_on, "unverified",
                    "{} claims a verification that has not happened",
                    preset.id
                );
            }
        }
    }

    #[test]
    fn local_runtimes_carry_no_data_handling_label() {
        // Nothing leaves the machine, so there is nothing to characterise.
        // A label here would imply a transfer that does not occur.
        for id in ["ollama", "lmstudio"] {
            assert!(preset(id).expect("preset").data_handling.is_none());
        }
    }

    #[test]
    fn an_unreachable_endpoint_is_unreachable_rather_than_malformed() {
        // The shapes have to be right or the fallback policy switches on the
        // wrong thing. Port 1 on loopback refuses immediately.
        let mut backend =
            CloudBackend::new("Test", "http://127.0.0.1:1/v1", "m", None).expect("builds");
        let error = backend
            .generate(
                &Request {
                    system: "rules".into(),
                    user: "text".into(),
                },
                &Cancel::new(),
            )
            .expect_err("cannot connect");
        assert!(
            matches!(error, BackendError::Unreachable(_)),
            "got {error:?}"
        );
    }

    #[test]
    fn a_cancelled_request_is_never_sent() {
        // Not merely reported as cancelled after the fact: an Operator who
        // stopped before it went out must have nothing go out.
        let cancel = Cancel::new();
        cancel.cancel();
        let mut backend =
            CloudBackend::new("Test", "https://api.example.invalid/v1", "m", None).expect("builds");
        let error = backend
            .generate(
                &Request {
                    system: "rules".into(),
                    user: "a transcript".into(),
                },
                &cancel,
            )
            .expect_err("cancelled");
        assert!(matches!(error, BackendError::Cancelled), "got {error:?}");
    }

    #[test]
    fn the_custom_endpoint_says_the_rules_are_the_operators() {
        assert_eq!(CUSTOM_ENDPOINT_LABEL, "unknown endpoint — your rules");
    }
}
