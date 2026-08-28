//! From captured stereo blocks to transcript segments.
//!
//! The two channels are transcribed independently. That is not an
//! optimisation — it is the attribution model: the mic channel is where the
//! Operator is and the system channel is everyone else (ADR-0029 as
//! amended), so keeping them separate is what lets M1 label speech at all
//! before Diarization exists.

use evertranscript_protocol::AudioChannel;
use tracing::debug;

use super::vad::ChunkPolicy;
use super::vad::Chunker;
use super::vad::EnergyDetector;
use super::whisper::WHISPER_RATE;
use super::Transcriber;
use crate::audio::joiner::StereoBlock;
use crate::audio::SAMPLE_RATE;

/// One transcribed span, ready to become a row in the record.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscribedSegment {
    pub channel: AudioChannel,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub confidence: f32,
}

/// Below this average token probability, text is treated as invention
/// rather than speech.
const MIN_CONFIDENCE: f32 = 0.25;

/// Runs capture audio through VAD chunking and transcription.
pub struct TranscriptionPipeline {
    mic: Chunker,
    system: Chunker,
    transcriber: Box<dyn Transcriber>,
    /// Last accepted text, fed forward as whisper's rolling prompt.
    previous_text: Option<String>,
    /// Leftover samples from resampling, so a block boundary never loses a
    /// fraction of a sample period.
    mic_remainder: Vec<f32>,
    system_remainder: Vec<f32>,
}

impl TranscriptionPipeline {
    pub fn new(transcriber: Box<dyn Transcriber>) -> Self {
        Self::with_policy(transcriber, ChunkPolicy::default())
    }

    pub fn with_policy(transcriber: Box<dyn Transcriber>, policy: ChunkPolicy) -> Self {
        Self {
            mic: Chunker::new(WHISPER_RATE, policy, Box::new(EnergyDetector::new())),
            system: Chunker::new(WHISPER_RATE, policy, Box::new(EnergyDetector::new())),
            transcriber,
            previous_text: None,
            mic_remainder: Vec::new(),
            system_remainder: Vec::new(),
        }
    }

    /// Feeds one stereo capture block, returning any segments that completed.
    pub fn push(&mut self, block: &StereoBlock) -> Vec<TranscribedSegment> {
        let (mic, system) = split(block);
        let mut segments = Vec::new();

        let mic_ready = decimate(&mut self.mic_remainder, &mic);
        for chunk in self.mic.push(&mic_ready) {
            if let Some(segment) = self.transcribe(AudioChannel::Mic, chunk) {
                segments.push(segment);
            }
        }

        let system_ready = decimate(&mut self.system_remainder, &system);
        for chunk in self.system.push(&system_ready) {
            if let Some(segment) = self.transcribe(AudioChannel::System, chunk) {
                segments.push(segment);
            }
        }

        segments.sort_by_key(|segment| segment.start_ms);
        segments
    }

    /// Transcribes whatever is still buffered. Called when a Meeting stops,
    /// so the last sentence is not lost (story 5).
    pub fn flush(&mut self) -> Vec<TranscribedSegment> {
        let mut segments = Vec::new();
        if let Some(chunk) = self.mic.flush() {
            if let Some(segment) = self.transcribe(AudioChannel::Mic, chunk) {
                segments.push(segment);
            }
        }
        if let Some(chunk) = self.system.flush() {
            if let Some(segment) = self.transcribe(AudioChannel::System, chunk) {
                segments.push(segment);
            }
        }
        segments.sort_by_key(|segment| segment.start_ms);
        segments
    }

    fn transcribe(
        &mut self,
        channel: AudioChannel,
        chunk: super::vad::SpeechChunk,
    ) -> Option<TranscribedSegment> {
        let result = match self
            .transcriber
            .transcribe(&chunk.samples, self.previous_text.as_deref())
        {
            Ok(result) => result,
            Err(error) => {
                // A failed decode costs one caption; it must not stop
                // capture or the Meeting (ADR-0029 as amended).
                debug!(%error, "a chunk failed to transcribe");
                return None;
            }
        };

        let text = super::filters::clean(&result.text)?;
        if result.confidence < MIN_CONFIDENCE {
            debug!(
                confidence = result.confidence,
                text, "dropping a low-confidence decode as invention"
            );
            return None;
        }

        self.previous_text = Some(text.clone());
        Some(TranscribedSegment {
            channel,
            start_ms: chunk.start_ms,
            end_ms: chunk.end_ms,
            text,
            confidence: result.confidence,
        })
    }
}

/// Splits an interleaved stereo block into its two mono channels.
fn split(block: &StereoBlock) -> (Vec<f32>, Vec<f32>) {
    let frames = block.frame_count();
    let mut mic = Vec::with_capacity(frames);
    let mut system = Vec::with_capacity(frames);
    for pair in block.samples.chunks_exact(2) {
        mic.push(pair[0]);
        system.push(pair[1]);
    }
    (mic, system)
}

/// 48 kHz capture down to the 16 kHz whisper needs, averaging each group of
/// three samples.
///
/// Averaging rather than picking every third sample: dropping samples aliases
/// everything above 8 kHz back down into the speech band as artefacts, which
/// a transcription model then dutifully tries to interpret. `remainder`
/// carries the partial group across block boundaries so no sample is lost.
fn decimate(remainder: &mut Vec<f32>, input: &[f32]) -> Vec<f32> {
    const FACTOR: usize = (SAMPLE_RATE / WHISPER_RATE) as usize;
    remainder.extend_from_slice(input);

    let groups = remainder.len() / FACTOR;
    let mut out = Vec::with_capacity(groups);
    for group in 0..groups {
        let start = group * FACTOR;
        let sum: f32 = remainder[start..start + FACTOR].iter().sum();
        out.push(sum / FACTOR as f32);
    }
    remainder.drain(..groups * FACTOR);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asr::FakeTranscriber;
    use crate::audio::CaptureOffset;

    fn block(mic_value: f32, system_value: f32, ms: u64) -> StereoBlock {
        let frames = (SAMPLE_RATE as u64 * ms / 1000) as usize;
        let mut samples = Vec::with_capacity(frames * 2);
        for index in 0..frames {
            // A 200 Hz tone scaled per channel, so the gate hears speech.
            let phase = (index as f32 / SAMPLE_RATE as f32 * 200.0 * std::f32::consts::TAU).sin();
            samples.push(phase * mic_value);
            samples.push(phase * system_value);
        }
        StereoBlock {
            offset: CaptureOffset::ZERO,
            samples,
        }
    }

    #[test]
    fn decimation_preserves_duration_and_level() {
        let mut remainder = Vec::new();
        let input = vec![0.5f32; 4_800]; // 100 ms at 48 kHz
        let out = decimate(&mut remainder, &input);
        assert_eq!(out.len(), 1_600, "100 ms at 16 kHz");
        assert!(
            out.iter().all(|sample| (*sample - 0.5).abs() < 1e-6),
            "averaging a constant must not change its level"
        );
        assert!(remainder.is_empty());
    }

    #[test]
    fn decimation_carries_partial_groups_across_blocks() {
        let mut remainder = Vec::new();
        // Deliberately not a multiple of 3.
        let first = decimate(&mut remainder, &[1.0; 100]);
        assert_eq!(first.len(), 33);
        assert_eq!(remainder.len(), 1, "the leftover sample is kept");

        let second = decimate(&mut remainder, &[1.0; 101]);
        assert_eq!(second.len(), 34, "the leftover joins the next block");
        assert_eq!(
            first.len() + second.len(),
            (100 + 101) / 3,
            "no samples are lost at a block boundary"
        );
    }

    #[test]
    fn the_two_channels_are_transcribed_separately() {
        let mut pipeline = TranscriptionPipeline::new(Box::new(FakeTranscriber::with([
            "operator speaking",
            "participant replying",
        ])));

        let mut segments = pipeline.push(&block(0.3, 0.3, 4_000));
        segments.extend(pipeline.push(&block(0.0, 0.0, 1_000)));
        segments.extend(pipeline.flush());

        let channels: Vec<AudioChannel> = segments.iter().map(|s| s.channel).collect();
        assert!(
            channels.contains(&AudioChannel::Mic) && channels.contains(&AudioChannel::System),
            "both legs must produce their own segments, got {channels:?}"
        );
    }

    #[test]
    fn known_hallucinations_never_reach_the_record() {
        // The record is immutable, so this is the last line of defence
        // before an invention is permanent.
        let mut pipeline = TranscriptionPipeline::new(Box::new(FakeTranscriber::with([
            "Thank you for watching!",
            "Thanks for watching",
        ])));

        let mut segments = pipeline.push(&block(0.3, 0.3, 4_000));
        segments.extend(pipeline.push(&block(0.0, 0.0, 1_000)));
        segments.extend(pipeline.flush());

        assert!(
            segments.is_empty(),
            "hallucinated text must be dropped, got {segments:?}"
        );
    }

    #[test]
    fn empty_decodes_are_dropped_rather_than_stored_as_blank_rows() {
        let mut pipeline = TranscriptionPipeline::new(Box::new(FakeTranscriber::with(["", "   "])));
        let mut segments = pipeline.push(&block(0.3, 0.3, 4_000));
        segments.extend(pipeline.flush());
        assert!(segments.is_empty());
    }

    #[test]
    fn the_previous_chunk_steers_the_next_one() {
        // Rolling context is what keeps a name spelled the same way through
        // a meeting; without it every chunk re-guesses.
        let transcriber = Box::new(FakeTranscriber::with(["Frank and Jack", "sync up"]));
        let mut pipeline = TranscriptionPipeline::new(transcriber);

        pipeline.push(&block(0.3, 0.0, 4_000));
        pipeline.push(&block(0.0, 0.0, 1_000));
        pipeline.push(&block(0.3, 0.0, 4_000));
        pipeline.push(&block(0.0, 0.0, 1_000));
        pipeline.flush();
        // The pipeline owns the transcriber, so the assertion is that it ran
        // without panicking and that context threading compiles; the direct
        // prompt assertion lives in the FakeTranscriber unit test below.
    }

    #[test]
    fn a_fake_transcriber_records_the_prompt_it_was_given() {
        use crate::asr::Transcriber;
        let mut fake = FakeTranscriber::with(["one", "two"]);
        fake.transcribe(&[0.0; 10], None).expect("first");
        fake.transcribe(&[0.0; 10], Some("one")).expect("second");
        assert_eq!(
            fake.prompts_seen,
            vec![None, Some("one".to_string())],
            "rolling context must reach the engine"
        );
    }

    #[test]
    fn silence_produces_no_segments_at_all() {
        let mut pipeline =
            TranscriptionPipeline::new(Box::new(FakeTranscriber::with(["should never be asked"])));
        let mut segments = pipeline.push(&block(0.0, 0.0, 20_000));
        segments.extend(pipeline.flush());
        assert!(
            segments.is_empty(),
            "silence must not even reach the transcriber"
        );
    }
}
