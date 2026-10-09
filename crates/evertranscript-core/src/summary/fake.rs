//! A Backend that answers from a script instead of a model.
//!
//! Every Knob, fallback, prompt-armor and map-reduce test drives this. The
//! reasons are the same as for the three seams before it — no 2 GB model, no
//! network, no minutes of waiting — plus one specific to M4: **the fallback
//! tests need to cause failures on purpose**, and a real Backend cannot be
//! asked to return a 401 on the third call.
//!
//! Two capabilities here exist because of specific mistakes they prevent:
//!
//! - It **records the prompts it was given**, so the armor tests in ticket
//!   03 can assert on what was actually sent rather than on what the calling
//!   code believed it was sending. Those are different, and the difference
//!   is where injections live.
//! - It can fail in **each** shape of [`BackendError`]. A fallback that only
//!   handles the failure its author imagined is the one that will not fire
//!   on the night it matters.

use std::sync::Arc;
use std::sync::Mutex;

use super::Backend;
use super::BackendError;
use super::BackendIdentity;
use super::Cancel;
use super::Request;

/// What the fake should do when asked to generate.
#[derive(Debug, Clone)]
pub enum Response {
    /// Produce this text.
    Text(String),
    /// Fail this way.
    Fails(Failure),
}

/// The failure shapes, as data.
///
/// [`BackendError`] is not `Clone` — it carries messages and wraps nothing —
/// so the script holds this and constructs the error on demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    Unreachable,
    Refused,
    TimedOut,
    Malformed,
    Unavailable,
}

impl Failure {
    fn into_error(self) -> BackendError {
        match self {
            Self::Unreachable => BackendError::Unreachable("connection refused".into()),
            Self::Refused => BackendError::Refused("401 unauthorized".into()),
            Self::TimedOut => BackendError::TimedOut("no response within 30s".into()),
            Self::Malformed => BackendError::Malformed("expected JSON, got HTML".into()),
            Self::Unavailable => BackendError::Unavailable("no model downloaded".into()),
        }
    }
}

/// A Backend that answers from a script.
pub struct FakeBackend {
    identity: BackendIdentity,
    script: Vec<Response>,
    calls: usize,
    /// Every request this Backend was handed, in order. Shared so a test can
    /// read it after the Backend has been moved into the code under test.
    seen: Arc<Mutex<Vec<Request>>>,
}

impl FakeBackend {
    /// A Backend that always returns the same text.
    pub fn returning(text: &str) -> Self {
        Self::scripted(
            BackendIdentity::LocalSidecar {
                model: "fake".into(),
            },
            vec![Response::Text(text.to_string())],
        )
    }

    /// A Backend that always fails the same way.
    pub fn failing(failure: Failure) -> Self {
        Self::scripted(
            BackendIdentity::LocalSidecar {
                model: "fake".into(),
            },
            vec![Response::Fails(failure)],
        )
    }

    /// A cloud Backend, for the tests where the distinction is the point.
    pub fn cloud(provider: &str, script: Vec<Response>) -> Self {
        Self::scripted(
            BackendIdentity::Cloud {
                provider: provider.to_string(),
                model: "fake".into(),
            },
            script,
        )
    }

    pub fn scripted(identity: BackendIdentity, script: Vec<Response>) -> Self {
        Self {
            identity,
            script,
            calls: 0,
            seen: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// A handle to what this Backend was asked to generate.
    ///
    /// Taken before the Backend is moved into the code under test, which is
    /// why it is a shared handle rather than a getter.
    pub fn prompts(&self) -> Arc<Mutex<Vec<Request>>> {
        Arc::clone(&self.seen)
    }

    /// How many times it was called — the map-reduce tests count chunks.
    pub fn calls(&self) -> usize {
        self.calls
    }
}

impl Backend for FakeBackend {
    fn generate(&mut self, request: &Request, cancel: &Cancel) -> Result<String, BackendError> {
        self.seen
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(request.clone());

        // The last entry repeats, so a script of one covers "always does
        // this" without a test having to know how many calls the code makes.
        let response = self
            .script
            .get(self.calls)
            .or_else(|| self.script.last())
            .cloned()
            .unwrap_or(Response::Text(String::new()));
        self.calls += 1;

        match response {
            Response::Text(text) => {
                if cancel.is_cancelled() {
                    return Err(BackendError::Cancelled);
                }
                Ok(text)
            }
            Response::Fails(failure) => Err(failure.into_error()),
        }
    }

    fn identity(&self) -> BackendIdentity {
        self.identity.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> Request {
        Request {
            system: "be helpful".into(),
            user: "a transcript".into(),
        }
    }

    #[test]
    fn calls_are_counted_so_map_reduce_can_be_asserted() {
        let mut backend = FakeBackend::returning("chunk summary");
        let cancel = Cancel::new();
        for _ in 0..3 {
            backend.generate(&request(), &cancel).expect("generates");
        }
        assert_eq!(backend.calls(), 3);
    }
}
