//! whisper.cpp behind thin, owned bindings (ADR-0014 as amended).
//!
//! The decode parameters here are not defaults — they are the settings both
//! shipping local notetakers converged on, and most of them exist to stop
//! the model inventing speech. That matters more for us than for them: our
//! record is immutable (ADR-0009), so a hallucinated "thank you for
//! watching" is not a glitch that scrolls away, it is a permanent line in
//! the Operator's History.

use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use tracing::debug;
use tracing::info;
use whisper_rs::FullParams;
use whisper_rs::SamplingStrategy;
use whisper_rs::WhisperContext;
use whisper_rs::WhisperContextParameters;

use super::Transcriber;
use super::Transcript;

/// Audio must reach whisper at exactly this rate.
pub const WHISPER_RATE: u32 = 16_000;

/// Language selection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Language {
    /// Let the model decide per chunk. The right default for the
    /// code-switching meetings this product is built for (story 7).
    #[default]
    Auto,
    /// An ISO code such as `en` or `zh`.
    Fixed(String),
}

impl Language {
    fn as_option(&self) -> Option<&str> {
        match self {
            Self::Auto => Some("auto"),
            Self::Fixed(code) => Some(code.as_str()),
        }
    }
}

/// whisper.cpp, loaded once and reused for every chunk.
pub struct WhisperEngine {
    context: WhisperContext,
    model_path: PathBuf,
    language: Language,
    threads: i32,
}

impl WhisperEngine {
    pub fn load(model_path: &Path) -> Result<Self> {
        Self::load_with(model_path, Language::default())
    }

    pub fn load_with(model_path: &Path, language: Language) -> Result<Self> {
        anyhow::ensure!(
            model_path.exists(),
            "no transcription model at {} — run `evertranscript models fetch`",
            model_path.display()
        );
        // whisper.cpp logs to stdout by default. The Core is a daemon whose
        // stdout is a log stream, so its chatter is routed into tracing
        // once, process-wide, instead of interleaving with our own output.
        static LOGGING: std::sync::Once = std::sync::Once::new();
        LOGGING.call_once(whisper_rs::install_logging_hooks);

        let started = std::time::Instant::now();
        let context =
            WhisperContext::new_with_params(model_path, WhisperContextParameters::default())
                .with_context(|| format!("loading the model at {}", model_path.display()))?;

        // Leave headroom: transcription runs while capture and encoding are
        // also on this machine, and starving them to decode faster trades a
        // caption's latency for the recording's integrity.
        let threads = (std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(4)
            / 2)
        .clamp(2, 8) as i32;

        info!(
            model = %model_path.display(),
            threads,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "transcription model loaded"
        );
        Ok(Self {
            context,
            model_path: model_path.to_path_buf(),
            language,
            threads,
        })
    }

    pub fn model_path(&self) -> &Path {
        &self.model_path
    }

    fn params(&self, previous: Option<&str>) -> FullParams<'_, '_> {
        // Greedy with best_of 1: sampling variety buys nothing here and
        // costs latency on every caption.
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(self.threads);
        params.set_language(self.language.as_option());
        params.set_translate(false);

        // Nothing may reach stdout: the Core is a daemon, and a library
        // printing to its stdout corrupts nothing here but is noise in logs.
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        // The anti-hallucination set.
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_no_context(previous.is_none());
        params.set_single_segment(false);

        // Deterministic decoding: temperature fallback re-rolls a chunk the
        // model found hard, and what it produces on the retry is more often
        // invention than correction.
        params.set_temperature(0.0);
        params.set_temperature_inc(0.2);
        params.set_entropy_thold(2.4);
        params.set_logprob_thold(-1.0);
        // Lowered from the 0.75 default, which rejects genuinely quiet
        // speech — the far-end talker on a bad connection.
        params.set_no_speech_thold(0.55);

        // Works around whisper.cpp's "single timestamp ending — skip entire
        // chunk" heuristic, which silently discards otherwise-valid
        // transcriptions. Timing comes from VAD chunk boundaries here, not
        // from the model, so nothing is lost by suppressing its timestamps.
        params.set_no_timestamps(true);
        params.set_token_timestamps(true);

        // Rolling context: the previous chunk's text steers the next one, so
        // a name or a piece of jargon transcribed once keeps its spelling
        // through the meeting.
        if let Some(previous) = previous {
            params.set_initial_prompt(previous);
        }
        params
    }
}

impl Transcriber for WhisperEngine {
    fn transcribe(&mut self, samples: &[f32], previous: Option<&str>) -> Result<Transcript> {
        if samples.is_empty() {
            return Ok(Transcript::default());
        }
        let started = std::time::Instant::now();
        let mut state = self
            .context
            .create_state()
            .context("creating a whisper state")?;

        state
            .full(self.params(previous), samples)
            .context("running transcription")?;

        let segment_count = state.full_n_segments();
        let mut text = String::new();
        let mut confidence_total = 0.0f32;
        let mut confidence_count = 0usize;
        let mut worst_no_speech = 0.0f32;

        for index in 0..segment_count {
            let Some(segment) = state.get_segment(index) else {
                continue;
            };
            let Ok(segment_text) = segment.to_str() else {
                continue;
            };
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(segment_text.trim());

            // The model's own verdict on whether this was speech at all.
            // When it says "probably not" but still emitted words, those
            // words are invented — the exact case that must not reach an
            // immutable record.
            worst_no_speech = worst_no_speech.max(segment.no_speech_probability());

            // Average token probability, used downstream to drop the
            // confident-sounding nonsense whisper emits on silence.
            for token_index in 0..segment.n_tokens() {
                if let Some(token) = segment.get_token(token_index) {
                    confidence_total += token.token_probability();
                    confidence_count += 1;
                }
            }
        }

        let confidence = if confidence_count > 0 {
            confidence_total / confidence_count as f32
        } else {
            0.0
        };
        // Fold no-speech into the confidence the pipeline filters on, so one
        // number carries both signals.
        let confidence = confidence * (1.0 - worst_no_speech).clamp(0.0, 1.0);
        let elapsed = started.elapsed();
        debug!(
            chars = text.len(),
            confidence,
            elapsed_ms = elapsed.as_millis() as u64,
            audio_ms = samples.len() as u64 * 1000 / WHISPER_RATE as u64,
            "transcribed a chunk"
        );

        Ok(Transcript {
            text: text.trim().to_string(),
            confidence,
            decode_time: elapsed,
        })
    }

    fn describe(&self) -> String {
        format!(
            "whisper.cpp ({})",
            self.model_path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown model".to_string())
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_model_is_a_legible_error_not_a_crash() {
        match WhisperEngine::load(Path::new("/nonexistent/model.bin")) {
            Ok(_) => panic!("loading a missing model must fail"),
            Err(error) => {
                let message = error.to_string();
                assert!(
                    message.contains("models fetch"),
                    "the error should say how to fix it: {message}"
                );
            }
        }
    }
}
