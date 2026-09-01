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
use std::sync::mpsc::Receiver;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::mpsc::channel;
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
///
/// **This was declared and never enforced.** `exchange` blocked in
/// `read_line` with no deadline, so the bound the catalog specifies existed
/// only as documentation — `cloud::REQUEST_TIMEOUT` was applied to its
/// request and this one was applied to nothing. A sidecar that loaded a
/// model and then stopped answering wedged its caller for as long as the
/// child lived. Clippy cannot say so, because a `pub` constant nobody reads
/// is not dead code (DECISIONS Q46).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(900);

/// How long a cancelled child gets to exit before it is killed outright.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

/// How to drive the loaded model, as it travels to the sidecar.
///
/// A mirror of the registry's `Driving` with owned strings: the registry's
/// version is `&'static str` because it is a compile-time table, and that
/// does not serialize into a message.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Driving {
    pub framing: String,
    pub sampling: Sampling,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suppress_reasoning: Option<String>,
    pub context_tokens: u32,
}

/// The sampling half, on the wire.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Sampling {
    Greedy,
    Nucleus {
        temperature: f32,
        top_p: f32,
        top_k: i32,
        min_p: f32,
    },
}

impl Default for Driving {
    /// What the sidecar did before a model could describe itself.
    ///
    /// Not a recommendation — a record of the previous behaviour, so a
    /// `Load` that carries no properties changes nothing.
    fn default() -> Self {
        Self {
            framing: "plain".to_string(),
            sampling: Sampling::Greedy,
            suppress_reasoning: None,
            context_tokens: 8_192,
        }
    }
}

impl Driving {
    /// From the registry's table into something sendable.
    pub fn from_entry(driving: &crate::models::registry::Driving) -> Self {
        use crate::models::registry::Framing;
        use crate::models::registry::Sampling as Registry;
        Self {
            framing: match driving.framing {
                Framing::Plain => "plain".to_string(),
                Framing::EmbeddedChatTemplate => "chatTemplate".to_string(),
            },
            sampling: match driving.sampling {
                Registry::Greedy => Sampling::Greedy,
                Registry::Nucleus {
                    temperature,
                    top_p,
                    top_k,
                    min_p,
                } => Sampling::Nucleus {
                    temperature,
                    top_p,
                    top_k,
                    min_p,
                },
            },
            suppress_reasoning: driving.suppress_reasoning.map(str::to_string),
            context_tokens: driving.context_tokens,
        }
    }
}

/// What the Core sends the sidecar.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
    ///
    /// Carries how to drive it. The sidecar could not read this from the
    /// registry — it is a separate process by design (ADR-0031) and the
    /// registry is the Core's — so the properties travel with the path.
    Load {
        model_path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        driving: Option<Driving>,
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
    /// Lines the child has written, delivered by a reader thread.
    ///
    /// **A thread rather than a direct `read_line` because a pipe read
    /// cannot be given a deadline portably**, and a deadline is the whole
    /// point: `recv_timeout` on this channel is what makes `REQUEST_TIMEOUT`
    /// real. The thread ends when the pipe reaches EOF, so a `Disconnected`
    /// here means the same thing `read_line` returning `Ok(0)` used to — the
    /// child is gone.
    lines: Receiver<std::io::Result<String>>,
    model: String,
    /// Bound on one exchange. `REQUEST_TIMEOUT` outside tests; a test that
    /// waited fifteen minutes to prove a timeout works would never be run.
    request_timeout: Duration,
}

impl SidecarBackend {
    /// Spawns the sidecar and loads a model.
    ///
    /// **stdin stays open for the life of the Backend**, which is the
    /// catalog's orphan protection: if the Core dies, the pipe closes, the
    /// child reads EOF and exits. Without it a crashed Core leaves a model
    /// resident in memory with nobody to stop it.
    pub fn spawn(binary: &std::path::Path, model_path: &str) -> Result<Self, BackendError> {
        Self::spawn_with_timeout(binary, model_path, REQUEST_TIMEOUT, None)
    }

    /// As `spawn`, telling the sidecar how this model wants to be driven.
    ///
    /// Separate from `spawn` rather than replacing it: a caller with no
    /// registry entry to hand — the tests that drive a stub sidecar — should
    /// not have to invent one, and omitting the properties means the sidecar
    /// falls back to what it did before they existed.
    pub fn spawn_driven(
        binary: &std::path::Path,
        model_path: &str,
        driving: Option<Driving>,
    ) -> Result<Self, BackendError> {
        Self::spawn_with_timeout(binary, model_path, REQUEST_TIMEOUT, driving)
    }

    /// As `spawn`, with the exchange bound named.
    ///
    /// Private: nothing in the Core wants a bound other than the catalog's.
    /// It exists so the timeout can be *tested*, which the 900-second one
    /// cannot be.
    fn spawn_with_timeout(
        binary: &std::path::Path,
        model_path: &str,
        request_timeout: Duration,
        driving: Option<Driving>,
    ) -> Result<Self, BackendError> {
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

        // Detached on purpose. It blocks in `read_line` and ends at EOF, and
        // every path that abandons a sidecar — `shutdown`, `Drop`, a timeout
        // — kills the child, which is what produces that EOF. There is no
        // state to join for.
        let (sender, lines) = channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        if sender.send(Ok(line)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(Err(error));
                        break;
                    }
                }
            }
        });

        let mut backend = Self {
            child,
            stdin: Some(stdin),
            lines,
            model: String::new(),
            request_timeout,
        };

        match backend.exchange(&SidecarRequest::Load {
            model_path: model_path.to_string(),
            driving,
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

    /// One request, one response, **bounded by `request_timeout`**.
    ///
    /// The bound is not decoration. A child that crashes is easy — the pipe
    /// EOFs and the wait ends — and that case had a test. A child that stays
    /// alive holding half a gigabyte and simply stops answering produces no
    /// EOF at all, and an unbounded read waits for it forever. That is the
    /// shape ADR-0031 bought the process boundary to survive, and it was the
    /// one shape this could not.
    fn exchange(&mut self, request: &SidecarRequest) -> Result<SidecarResponse, BackendError> {
        let line = serde_json::to_string(request)
            .map_err(|error| BackendError::Malformed(error.to_string()))?;
        let stdin = self
            .stdin
            .as_mut()
            // Covers both a `shutdown` in progress and a child this method
            // has already killed for missing its deadline.
            .ok_or_else(|| BackendError::Unreachable("the sidecar is no longer running".into()))?;
        writeln!(stdin, "{line}")
            .and_then(|()| stdin.flush())
            .map_err(|error| BackendError::Unreachable(error.to_string()))?;

        let answer = match self.lines.recv_timeout(self.request_timeout) {
            Ok(Ok(answer)) => answer,
            Ok(Err(error)) => return Err(BackendError::Unreachable(error.to_string())),
            // The reader thread ended, which it only does at EOF: the child
            // is gone. Distinguished from a malformed reply because they
            // call for different responses — one is a crashed sidecar to
            // restart, the other is a protocol bug.
            Err(RecvTimeoutError::Disconnected) => {
                return Err(BackendError::Unreachable("the sidecar exited".into()));
            }
            Err(RecvTimeoutError::Timeout) => {
                // **Kill, don't ask.** A child past its deadline is either
                // wedged or in a decode that cannot be interrupted, and
                // both are the case `shutdown`'s comment describes: asking
                // politely and waiting is not a stop. Leaving it alive
                // would also leave the model resident with nobody to end
                // it, which is the orphan the stdin pipe exists to prevent.
                self.stdin = None;
                let _ = self.child.kill();
                let _ = self.child.wait();
                return Err(BackendError::Unreachable(format!(
                    "the sidecar did not answer within {}s and was killed",
                    self.request_timeout.as_secs()
                )));
            }
        };
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
    fn a_sidecar_that_stops_answering_is_killed_rather_than_waited_on() {
        // The gap the test above leaves. That child *dies*, and dying is
        // what makes it detectable — the pipe EOFs and the read ends. This
        // one loads its model and then goes silent while staying alive, so
        // there is no EOF to notice, and an unbounded read waits for it as
        // long as the process lives.
        //
        // `REQUEST_TIMEOUT` was declared for exactly this case and applied
        // to nothing, so until now the only bound on a silent sidecar was
        // the lifetime of the process (DECISIONS Q46).
        let path = fake_sidecar(
            r##"#!/bin/sh
IFS= read -r line
printf '%s\n' '{"type":"ready","model":"silent"}'
while true; do sleep 1; done
"##,
        );
        // Five seconds, not one. The same bound covers the `load` exchange,
        // and that one is *supposed* to answer — a bound tight enough to
        // race a shell's startup under sixteen parallel tests would make
        // this flaky in the direction of failing when the code is right,
        // which is the worst way for a test about hangs to behave.
        let mut backend = SidecarBackend::spawn_with_timeout(
            &path,
            "/models/fake.gguf",
            Duration::from_secs(5),
            None,
        )
        .expect("spawns and loads");

        let started = Instant::now();
        let error = backend
            .generate(
                &Request {
                    system: "rules".into(),
                    user: "text".into(),
                },
                &Cancel::new(),
            )
            .expect_err("a silent sidecar must not be waited on forever");
        assert!(
            matches!(error, BackendError::Unreachable(_)),
            "got {error:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the deadline did not fire: waited {:?}",
            started.elapsed()
        );

        // **And the child is gone.** Returning an error while leaving a
        // process holding the model resident would trade a visible hang for
        // an invisible leak, which is the worse of the two.
        assert!(
            backend.child.try_wait().expect("try_wait").is_some(),
            "the timed-out sidecar is still alive"
        );

        // A dead Backend says so rather than blocking again.
        assert!(matches!(
            backend.exchange(&SidecarRequest::Ping),
            Err(BackendError::Unreachable(_))
        ));
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
    #[test]
    fn the_registered_model_is_driven_exactly_as_the_sidecar_used_to_drive_it() {
        // **The guard on introducing this seam**: describing the model must
        // change no output, and the way to know that is that its description
        // equals the behaviour the sidecar had before descriptions existed.
        //
        // This test is expected to fail when the registered model changes,
        // and that failure is the point — it is the moment someone must
        // decide the new behaviour deliberately. Whoever swaps the model
        // should replace this with an assertion about what the new model
        // wants, not delete it.
        let described = Driving::from_entry(
            crate::models::registry::SUMMARY_DEFAULT
                .driving
                .as_ref()
                .expect("the Summary model is prompted"),
        );
        assert_eq!(
            described,
            Driving::default(),
            "the registered model is described differently from how the sidecar \
             behaved before it could be described — which means this change is \
             not the no-op it claims to be"
        );
    }
}
