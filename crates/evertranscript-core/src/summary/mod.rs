//! Summary: the one feature where the Operator may choose the cloud.
//!
//! The fourth seam in this codebase, and the shape is deliberate by now —
//! AudioSource, DetectionSource, Diarizer, and now [`Backend`]. Each one
//! exists because the decisions worth testing are policy, and pinning policy
//! to hardware, a model, or a network makes it untestable.
//!
//! This one carries a weight the others do not. Transcription and
//! Diarization are Anchors (ADR-0002): permanently local, no Knob, no
//! discussion. Summary is the single exception, and ADR-0034 lists the cloud
//! Summary Backend as one of exactly three things this product may ever say
//! on the wire. **Everything in this module is downstream of the fact that a
//! bug here can send a meeting transcript to a stranger.**
//!
//! Two consequences show up immediately in the types:
//!
//! 1. **Failures are typed by shape, not by message.** The fallback policy
//!    switches on [`BackendError`], and a policy that matched on strings
//!    would break the first time a provider reworded an error — quietly, in
//!    the direction of not falling back.
//! 2. **A Backend says which one it is.** Story 38 requires the *active*
//!    Backend to be visible, not merely the configured one, so
//!    [`Backend::identity`] is part of the contract rather than a logging
//!    nicety.

pub mod fake;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

/// Where generated text came from.
///
/// Carried on the Summary and shown beside it: an Operator who chose Cloud
/// and received local quality is owed the reason, and an Operator who chose
/// Local is owed proof that is what ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendIdentity {
    /// The bundled llama.cpp sidecar (ADR-0031).
    LocalSidecar { model: String },
    /// A local OpenAI-compatible runtime the Operator already had — Ollama,
    /// LM Studio. Local, but not ours, so it is named separately: "local"
    /// is a claim about where the data went, and the Operator should be able
    /// to see which local thing it went to.
    LocalRuntime { name: String, model: String },
    /// A cloud provider the Operator explicitly chose.
    Cloud { provider: String, model: String },
}

impl BackendIdentity {
    /// Whether using this sends meeting content off the machine.
    ///
    /// The question the whole milestone turns on, so it is one method rather
    /// than a `match` repeated at each call site — each of which would be a
    /// place to get it wrong.
    pub fn leaves_the_machine(&self) -> bool {
        matches!(self, Self::Cloud { .. })
    }

    /// What to show an Operator.
    pub fn label(&self) -> String {
        match self {
            Self::LocalSidecar { model } => format!("Local ({model})"),
            Self::LocalRuntime { name, model } => format!("{name} ({model})"),
            Self::Cloud { provider, model } => format!("{provider} ({model})"),
        }
    }
}

/// Why generation did not produce text.
///
/// **Shapes, not messages.** Ticket 07's fallback switches on these, and the
/// set is deliberately small: a provider can invent any prose it likes, and
/// the policy must still be able to tell "your key is wrong" from "the
/// network is down", because those deserve different answers.
#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    /// Nothing answered: no network, wrong host, sidecar not running.
    #[error("backend unreachable: {0}")]
    Unreachable(String),
    /// Something answered and said no: a bad key, an exhausted quota, a
    /// model the account cannot use. Retrying will not help and falling back
    /// might.
    #[error("backend refused the request: {0}")]
    Refused(String),
    /// Answered too slowly, or stopped answering part way through.
    #[error("backend timed out: {0}")]
    TimedOut(String),
    /// Answered with something that is not what it promised.
    #[error("backend returned an unusable response: {0}")]
    Malformed(String),
    /// The Operator stopped it. Not a failure, and specifically **not** a
    /// reason to fall back: falling back on cancel would take an Operator
    /// who pressed stop and send their transcript to a provider instead.
    #[error("generation cancelled")]
    Cancelled,
    /// Nothing to run: no model downloaded, no key set, no Backend chosen.
    #[error("backend unavailable: {0}")]
    Unavailable(String),
}

impl BackendError {
    /// Whether a different Backend is worth trying.
    ///
    /// Cancellation is excluded, and that exclusion is the point: every
    /// other variant is the machinery failing, while `Cancelled` is the
    /// Operator succeeding at stopping it.
    pub fn is_worth_falling_back_from(&self) -> bool {
        !matches!(self, Self::Cancelled)
    }
}

/// What to generate.
///
/// A struct rather than two string arguments because the two are not
/// interchangeable and swapping them is silent: the transcript would become
/// the instructions, which is precisely the injection the armor in ticket 03
/// exists to prevent — self-inflicted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// The rules. Operator-editable (story 42), with a default.
    pub system: String,
    /// The material — transcript, Notes, speakers. **Untrusted**: everyone
    /// who spoke in the meeting wrote part of it.
    pub user: String,
}

/// A cancellation flag a running generation checks.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Produces generated text.
pub trait Backend: Send {
    /// Runs to completion, cancellation, or failure.
    fn generate(&mut self, request: &Request, cancel: &Cancel) -> Result<String, BackendError>;

    /// Which Backend this is — for the indicator story 38 requires.
    fn identity(&self) -> BackendIdentity;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_cloud_backend_leaves_the_machine() {
        // The question the milestone turns on, answered in one place so
        // there is one place to get it wrong rather than one per call site.
        assert!(
            !BackendIdentity::LocalSidecar {
                model: "qwen".into()
            }
            .leaves_the_machine()
        );
        assert!(
            !BackendIdentity::LocalRuntime {
                name: "Ollama".into(),
                model: "llama".into()
            }
            .leaves_the_machine(),
            "someone else's local runtime is still local"
        );
        assert!(
            BackendIdentity::Cloud {
                provider: "OpenAI".into(),
                model: "gpt".into()
            }
            .leaves_the_machine()
        );
    }

    #[test]
    fn cancelling_is_never_a_reason_to_try_a_different_backend() {
        // An Operator who pressed stop must not have their transcript sent
        // to a provider as the consequence. Every other shape is machinery
        // failing; this one is the Operator succeeding.
        assert!(!BackendError::Cancelled.is_worth_falling_back_from());

        for failure in [
            BackendError::Unreachable("no route".into()),
            BackendError::Refused("401".into()),
            BackendError::TimedOut("30s".into()),
            BackendError::Malformed("not json".into()),
            BackendError::Unavailable("no model".into()),
        ] {
            assert!(
                failure.is_worth_falling_back_from(),
                "{failure:?} should be recoverable"
            );
        }
    }

    #[test]
    fn a_backend_can_say_what_it_is_to_a_person() {
        // Story 38 wants the *active* Backend visible. "Local" alone is not
        // enough — an Operator running Ollama and an Operator running the
        // bundled sidecar are in different situations.
        assert_eq!(
            BackendIdentity::LocalSidecar {
                model: "qwen2.5-3b".into()
            }
            .label(),
            "Local (qwen2.5-3b)"
        );
        assert_eq!(
            BackendIdentity::Cloud {
                provider: "Anthropic".into(),
                model: "claude".into()
            }
            .label(),
            "Anthropic (claude)"
        );
    }

    #[test]
    fn a_cancelled_flag_is_visible_to_the_thread_doing_the_work() {
        let cancel = Cancel::new();
        let watcher = cancel.clone();
        assert!(!watcher.is_cancelled());
        cancel.cancel();
        assert!(watcher.is_cancelled());
    }
}
