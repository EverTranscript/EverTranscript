//! The local Summary Backend: a supervised child process speaking JSONL.
//!
//! ADR-0031 wants "a small local model is the default Backend" to hold on a
//! fresh machine with nothing else installed, and it wants the engine in its
//! own process. The second half is the interesting one, and the ADR's
//! evidence is specific: the competitor who embedded llama.cpp **in-process**
//! abandoned that path. The Core is the thing that must never die, because
//! it is the thing that is recording — and a Summary is the lowest-value
//! work this product does. Trading a live recording for it is the worst
//! trade available, so the model runs behind a process boundary that can
//! crash without taking the Core with it.
//!
//! This module is the Core's side: the protocol, the supervision, and the
//! lifecycle constants from the catalog. The child that embeds llama.cpp is
//! a separate binary; everything here is testable against any executable
//! that speaks the protocol, which is how it is tested.

use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;
use std::time::Instant;

use serde::Deserialize;
use serde::Serialize;

use super::Backend;
use super::BackendError;
use super::BackendIdentity;
use super::Cancel;
use super::Request;

/// Longest a single generation may take (catalog M4: 900 s).
///
/// Ninety minutes of meeting through a small model on a laptop is genuinely
/// slow, and a timeout tuned for a demo would fire on every real meeting.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(900);

/// How long a cancelled child gets to exit before it is killed outright.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

/// What the Core sends the sidecar.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
// `rename_all` renames *variants*; fields need their own attribute. Without
// the second one the wire is camelCase for the tag and snake_case for the
// fields, which both sides tolerate because they share this type — and which
// misleads the first human to hand-write a message while debugging. (It
// misled the author, on the first run against a real model.)
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SidecarRequest {
    /// Load a model, or confirm the loaded one matches.
    Load {
        model_path: String,
    },
    Generate {
        system: String,
        user: String,
    },
    /// Liveness. Skipped while a generation is running (catalog M4): a ping
    /// that queued behind a 900-second job would report the sidecar dead
    /// precisely when it was working hardest.
    Ping,
    Shutdown,
}

/// What the sidecar sends back.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SidecarResponse {
    Ready { model: String },
    Generated { text: String },
    Pong,
    Error { message: String },
}

/// A supervised local model process.
pub struct SidecarBackend {
    child: Child,
    /// An `Option` so shutdown can **drop** it: closing stdin is the EOF
    /// that tells the child to exit, and it is the only stop signal that
    /// works on a child which has stopped answering.
    stdin: Option<std::process::ChildStdin>,
    stdout: BufReader<std::process::ChildStdout>,
    model: String,
}

impl SidecarBackend {
    /// Spawns the sidecar and loads a model.
    ///
    /// **stdin stays open for the life of the Backend**, which is the
    /// catalog's orphan protection: if the Core dies, the pipe closes, the
    /// child reads EOF and exits. Without it a crashed Core leaves a model
    /// resident in memory with nobody to stop it.
    pub fn spawn(binary: &std::path::Path, model_path: &str) -> Result<Self, BackendError> {
        let mut child = Command::new(binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Inherited, so the child's diagnostics land in the Core's logs
            // rather than nowhere (catalog M4).
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| BackendError::Unavailable(format!("{}: {error}", binary.display())))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| BackendError::Unavailable("sidecar has no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| BackendError::Unavailable("sidecar has no stdout".into()))?;

        let mut backend = Self {
            child,
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
            model: String::new(),
        };

        match backend.exchange(&SidecarRequest::Load {
            model_path: model_path.to_string(),
        })? {
            SidecarResponse::Ready { model } => {
                backend.model = model;
                Ok(backend)
            }
            SidecarResponse::Error { message } => Err(BackendError::Unavailable(message)),
            other => Err(BackendError::Malformed(format!(
                "expected ready, got {other:?}"
            ))),
        }
    }

    /// One request, one response.
    fn exchange(&mut self, request: &SidecarRequest) -> Result<SidecarResponse, BackendError> {
        let line = serde_json::to_string(request)
            .map_err(|error| BackendError::Malformed(error.to_string()))?;
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| BackendError::Unreachable("the sidecar is shutting down".into()))?;
        writeln!(stdin, "{line}")
            .and_then(|()| stdin.flush())
            .map_err(|error| BackendError::Unreachable(error.to_string()))?;

        let mut answer = String::new();
        let read = self
            .stdout
            .read_line(&mut answer)
            .map_err(|error| BackendError::Unreachable(error.to_string()))?;
        if read == 0 {
            // EOF: the child is gone. Distinguished from a malformed reply
            // because they call for different responses — one is a crashed
            // sidecar to restart, the other is a protocol bug.
            return Err(BackendError::Unreachable("the sidecar exited".into()));
        }
        serde_json::from_str(answer.trim())
            .map_err(|error| BackendError::Malformed(format!("{error}: {}", answer.trim())))
    }

    /// Whether the sidecar is still answering.
    pub fn ping(&mut self) -> bool {
        matches!(
            self.exchange(&SidecarRequest::Ping),
            Ok(SidecarResponse::Pong)
        )
    }

    /// Ends the child: ask, wait briefly, then kill.
    ///
    /// **Kill-as-cancel is not a shortcut** (catalog M4): llama.cpp cannot be
    /// interrupted mid-generation, so a cancellation that politely asks and
    /// then waits is not a cancellation. The ask is for the idle case; the
    /// kill is what makes stop mean stop.
    pub fn shutdown(&mut self) {
        // Ask, but **never wait for an answer**. A child mid-generation
        // cannot reply — llama.cpp does not check for messages while it
        // decodes — so a shutdown that blocked on a response would hang
        // forever on exactly the child it most needs to stop. Found by this
        // hanging in its own test.
        if let Some(stdin) = self.stdin.as_mut() {
            let _ = writeln!(
                stdin,
                "{}",
                serde_json::to_string(&SidecarRequest::Shutdown).unwrap_or_default()
            );
            let _ = stdin.flush();
        }
        // Then close it. EOF on stdin is the catalog's orphan protection and
        // works on a child that has stopped reading messages but is still
        // watching its pipe.
        self.stdin = None;

        let deadline = Instant::now() + SHUTDOWN_GRACE;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                _ => break,
            }
        }
        // Kill-as-cancel. This is what makes stop mean stop.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for SidecarBackend {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Backend for SidecarBackend {
    fn generate(&mut self, request: &Request, cancel: &Cancel) -> Result<String, BackendError> {
        if cancel.is_cancelled() {
            return Err(BackendError::Cancelled);
        }
        match self.exchange(&SidecarRequest::Generate {
            system: request.system.clone(),
            user: request.user.clone(),
        })? {
            SidecarResponse::Generated { text } => Ok(text),
            SidecarResponse::Error { message } => Err(BackendError::Malformed(message)),
            other => Err(BackendError::Malformed(format!(
                "expected generated text, got {other:?}"
            ))),
        }
    }

    fn identity(&self) -> BackendIdentity {
        BackendIdentity::LocalSidecar {
            model: self.model.clone(),
        }
    }
}

/// Decodes UTF-8 arriving in pieces.
///
/// **A model emits tokens, not characters**, and a Chinese character is
/// three bytes that a tokenizer will happily split across two of them.
/// Decoding each piece independently turns 会 into two replacement
/// characters, permanently, in a record that is immutable by design.
///
/// This product has already paid for Chinese handling once (DECISIONS
/// Q11–Q13) and the transcripts it summarizes are routinely Chinese, which
/// is why this is a type rather than a `String::from_utf8_lossy` at the call
/// site.
#[derive(Debug, Default)]
pub struct IncrementalUtf8 {
    pending: Vec<u8>,
}

impl IncrementalUtf8 {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whatever is now complete. Bytes forming a partial character are held
    /// until the rest arrives.
    pub fn push(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        match std::str::from_utf8(&self.pending) {
            Ok(text) => {
                let text = text.to_string();
                self.pending.clear();
                text
            }
            Err(error) => {
                let good = error.valid_up_to();
                let text = String::from_utf8_lossy(&self.pending[..good]).into_owned();
                self.pending.drain(..good);
                text
            }
        }
    }

    /// Anything left at the end of a stream.
    ///
    /// Truncated bytes are lossy here and nowhere earlier: at the end there
    /// is no more input coming, so holding them forever would silently drop
    /// the last character of every generation.
    pub fn finish(&mut self) -> String {
        let text = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chinese_character_split_across_tokens_survives() {
        // The failure this exists to prevent, in its exact form: three bytes
        // of 会 arriving as 2 + 1. Decoded independently that is two
        // replacement characters, written permanently into a record that is
        // immutable by design.
        let full = "会议".as_bytes();
        let mut decoder = IncrementalUtf8::new();

        let mut out = String::new();
        out.push_str(&decoder.push(&full[..2]));
        assert_eq!(out, "", "an incomplete character emits nothing yet");
        out.push_str(&decoder.push(&full[2..]));
        out.push_str(&decoder.finish());

        assert_eq!(out, "会议");
        assert!(!out.contains('\u{fffd}'));
    }

    #[test]
    fn one_byte_at_a_time_still_produces_the_original_text() {
        // The worst case, and the one a streaming model actually produces.
        let original = "决定推迟投票 — and the ASCII too, plus an emoji 🎉";
        let mut decoder = IncrementalUtf8::new();
        let mut out = String::new();
        for byte in original.as_bytes() {
            out.push_str(&decoder.push(&[*byte]));
        }
        out.push_str(&decoder.finish());
        assert_eq!(out, original);
    }

    #[test]
    fn ascii_passes_straight_through_without_being_held() {
        // The common case must not be delayed by the machinery that exists
        // for the rare one.
        let mut decoder = IncrementalUtf8::new();
        assert_eq!(decoder.push(b"hello"), "hello");
    }

    #[test]
    fn a_truncated_stream_loses_only_the_broken_character() {
        // A sidecar killed mid-character. Everything complete is kept.
        let mut decoder = IncrementalUtf8::new();
        let bytes = "ok 会".as_bytes();
        let out = decoder.push(&bytes[..bytes.len() - 1]);
        assert_eq!(out, "ok ");
        assert!(decoder.finish().contains('\u{fffd}'), "and says it broke");
    }

    #[test]
    fn the_protocol_round_trips_as_jsonl() {
        // One request per line, one response per line — the property the
        // whole transport depends on. A payload containing a newline (every
        // transcript does) must not become two messages.
        let request = SidecarRequest::Generate {
            system: "rules".into(),
            user: "line one\nline two\n<transcript>".into(),
        };
        let line = serde_json::to_string(&request).expect("encodes");
        assert!(!line.contains('\n'), "a request must be one line");
        assert_eq!(
            serde_json::from_str::<SidecarRequest>(&line).expect("decodes"),
            request
        );
    }

    #[test]
    fn every_response_shape_round_trips() {
        for response in [
            SidecarResponse::Ready {
                model: "qwen".into(),
            },
            SidecarResponse::Generated {
                text: "# Summary\n\n会议决定".into(),
            },
            SidecarResponse::Pong,
            SidecarResponse::Error {
                message: "no model".into(),
            },
        ] {
            let line = serde_json::to_string(&response).expect("encodes");
            assert!(!line.contains('\n'));
            assert_eq!(
                serde_json::from_str::<SidecarResponse>(&line).expect("decodes"),
                response
            );
        }
    }

    #[test]
    fn a_missing_sidecar_binary_is_unavailable_rather_than_a_panic() {
        // A fresh install before the model is downloaded, and every machine
        // where the binary failed to ship. The Meeting must survive it.
        let result = SidecarBackend::spawn(
            std::path::Path::new("/nonexistent/evertranscript-summarizer"),
            "/nonexistent/model.gguf",
        );
        assert!(matches!(result, Err(BackendError::Unavailable(_))));
    }

    /// A stand-in sidecar: `sh` reading JSONL and answering it.
    ///
    /// The supervision is testable against anything that speaks the
    /// protocol, which is the point of having one.
    #[cfg(unix)]
    fn fake_sidecar(script: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("et-sidecar-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("sidecar.sh");
        std::fs::write(&path, script).expect("write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }

    #[cfg(unix)]
    #[test]
    fn a_well_behaved_sidecar_loads_generates_and_pings() {
        let path = fake_sidecar(
            r##"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"load"'*) printf '%s\n' '{"type":"ready","model":"fake-3b"}' ;;
    *'"generate"'*) printf '%s\n' '{"type":"generated","text":"# Summary\n\n会议决定推迟。"}' ;;
    *'"ping"'*) printf '%s\n' '{"type":"pong"}' ;;
    *'"shutdown"'*) exit 0 ;;
  esac
done
"##,
        );
        let mut backend = SidecarBackend::spawn(&path, "/models/fake.gguf").expect("spawns");
        assert_eq!(
            backend.identity(),
            BackendIdentity::LocalSidecar {
                model: "fake-3b".into()
            }
        );
        assert!(backend.ping());

        let text = backend
            .generate(
                &Request {
                    system: "rules".into(),
                    user: "a transcript".into(),
                },
                &Cancel::new(),
            )
            .expect("generates");
        assert!(text.contains("会议决定推迟"), "CJK survived the round trip");
    }

    #[cfg(unix)]
    #[test]
    fn a_sidecar_that_dies_is_unreachable_rather_than_a_hang() {
        // The crash-isolation case ADR-0031 bought the process boundary for.
        // The Core must notice and carry on, not block forever on a pipe.
        let path = fake_sidecar(
            r##"#!/bin/sh
IFS= read -r line
printf '%s\n' '{"type":"ready","model":"fake"}'
exit 1
"##,
        );
        let mut backend = SidecarBackend::spawn(&path, "/models/fake.gguf").expect("spawns");
        let error = backend
            .generate(
                &Request {
                    system: "rules".into(),
                    user: "text".into(),
                },
                &Cancel::new(),
            )
            .expect_err("the child is gone");
        assert!(
            matches!(error, BackendError::Unreachable(_)),
            "got {error:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_sidecar_that_cannot_load_its_model_says_so_rather_than_pretending() {
        let path = fake_sidecar(
            r##"#!/bin/sh
while IFS= read -r line; do
  printf '%s\n' '{"type":"error","message":"model file not found"}'
done
"##,
        );
        let result = SidecarBackend::spawn(&path, "/models/missing.gguf");
        assert!(matches!(result, Err(BackendError::Unavailable(_))));
    }

    #[cfg(unix)]
    #[test]
    fn a_cancelled_generation_never_reaches_the_model() {
        let path = fake_sidecar(
            r##"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"load"'*) printf '%s\n' '{"type":"ready","model":"fake"}' ;;
    *) printf '%s\n' '{"type":"generated","text":"should not happen"}' ;;
  esac
done
"##,
        );
        let mut backend = SidecarBackend::spawn(&path, "/models/fake.gguf").expect("spawns");
        let cancel = Cancel::new();
        cancel.cancel();
        assert!(matches!(
            backend.generate(
                &Request {
                    system: "r".into(),
                    user: "u".into()
                },
                &cancel
            ),
            Err(BackendError::Cancelled)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_sidecar_that_ignores_shutdown_is_killed() {
        // Kill-as-cancel: llama.cpp cannot be interrupted, so asking nicely
        // and waiting is not cancellation. This one refuses to leave.
        let path = fake_sidecar(
            r##"#!/bin/sh
IFS= read -r line
printf '%s\n' '{"type":"ready","model":"stubborn"}'
while true; do sleep 1; done
"##,
        );
        let mut backend = SidecarBackend::spawn(&path, "/models/fake.gguf").expect("spawns");
        let started = Instant::now();
        backend.shutdown();
        assert!(
            started.elapsed() < SHUTDOWN_GRACE + Duration::from_secs(5),
            "shutdown must not hang on a child that ignores it"
        );
    }
}
