//! Turning captured audio into a Transcript.
//!
//! Transcription is an Anchor: permanently local, with no Backend selector
//! (ADR-0002). There is no cloud path here to disable, because none exists —
//! that absence is the Closed Boundary, not a setting.

pub mod pipeline;
pub mod vad;
pub mod whisper;

use anyhow::Result;

/// What one decode produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Transcript {
    pub text: String,
    /// Average token probability. Low values on confident-looking text are
    /// the signature of a hallucination, so this is kept rather than
    /// discarded.
    pub confidence: f32,
    pub decode_time: std::time::Duration,
}

impl Transcript {
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }
}

/// The seam an ASR engine plugs into.
///
/// A trait rather than a concrete type so tests can transcribe without a
/// model — but deliberately *not* a provider abstraction: ADR-0002 keeps
/// transcription local, and a thirty-variant engine enum in this path is
/// exactly the sprawl the Anchor rule forbids.
pub trait Transcriber: Send {
    /// Transcribes 16 kHz mono audio. `previous` is the last chunk's text,
    /// used as rolling context.
    fn transcribe(&mut self, samples: &[f32], previous: Option<&str>) -> Result<Transcript>;

    fn describe(&self) -> String;
}

/// A Transcriber that returns canned text. Lets the pipeline, the delta
/// journal, and the caption channel be tested without loading a model.
#[derive(Default)]
pub struct FakeTranscriber {
    pub responses: std::collections::VecDeque<String>,
    /// Every call's `previous`, so tests can assert rolling context is wired.
    pub prompts_seen: Vec<Option<String>>,
}

impl FakeTranscriber {
    pub fn with(responses: impl IntoIterator<Item = &'static str>) -> Self {
        Self {
            responses: responses.into_iter().map(str::to_string).collect(),
            prompts_seen: Vec::new(),
        }
    }
}

impl Transcriber for FakeTranscriber {
    fn transcribe(&mut self, _samples: &[f32], previous: Option<&str>) -> Result<Transcript> {
        self.prompts_seen.push(previous.map(str::to_string));
        Ok(Transcript {
            text: self.responses.pop_front().unwrap_or_default(),
            confidence: 0.9,
            decode_time: std::time::Duration::from_millis(1),
        })
    }

    fn describe(&self) -> String {
        "fake".to_string()
    }
}
