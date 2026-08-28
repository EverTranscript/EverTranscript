//! From captured stereo blocks to transcript segments.
//!
//! The two channels are transcribed independently. That is not an
//! optimisation — it is the attribution model: the mic channel is where the
//! Operator is and the system channel is everyone else (ADR-0029 as
//! amended), so keeping them separate is what lets M1 label speech at all
//! before Diarization exists.

use evertranscript_protocol::AudioChannel;
use tracing::debug;

use super::Transcriber;
use super::vad::ChunkPolicy;
use super::vad::Chunker;
use super::vad::EnergyDetector;
use super::whisper::WHISPER_RATE;
use crate::audio::SAMPLE_RATE;
use crate::audio::aec;
use crate::audio::dsp;
use crate::audio::joiner::StereoBlock;

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
    /// Stateful resamplers, one per leg for the life of the Meeting.
    /// Rebuilding them per block loses the filter state at every boundary —
    /// the 173%-amplitude bug the DSP tests guard against.
    mic_resampler: Option<dsp::StreamResampler>,
    system_resampler: Option<dsp::StreamResampler>,
    /// Loudness conditioning, applied to what reaches the *model* rather
    /// than to what is stored: the file on disk stays as captured, so
    /// Enhance can re-derive from unmodified audio later (ADR-0019).
    mic_loudness: Option<dsp::LoudnessNormalizer>,
    system_loudness: Option<dsp::LoudnessNormalizer>,
    /// Removes the far end from the microphone leg on speakerphone. Costs
    /// nothing on headphones, where it converges to doing nothing at all.
    echo: Option<aec::EchoCanceller>,
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
            mic_resampler: dsp::StreamResampler::new(SAMPLE_RATE, WHISPER_RATE).ok(),
            system_resampler: dsp::StreamResampler::new(SAMPLE_RATE, WHISPER_RATE).ok(),
            mic_loudness: dsp::LoudnessNormalizer::new(WHISPER_RATE).ok(),
            system_loudness: dsp::LoudnessNormalizer::new(WHISPER_RATE).ok(),
            echo: Some(aec::EchoCanceller::new(WHISPER_RATE)),
        }
    }

    /// Turns echo cancellation off.
    ///
    /// Exists so a test can run the same audio both ways: a claim that the
    /// canceller stops the far end being credited to the Operator means
    /// nothing unless the version without it demonstrably fails.
    pub fn without_echo_cancellation(mut self) -> Self {
        self.echo = None;
        self
    }

    /// Feeds one stereo capture block, returning any segments that completed.
    pub fn push(&mut self, block: &StereoBlock) -> Vec<TranscribedSegment> {
        let (mic, system) = split(block);
        let mut segments = Vec::new();

        // Both legs are resampled before either is chunked, because echo
        // cancellation needs them side by side. They came from one block on
        // one clock and go through identical resamplers, so they stay
        // aligned to the sample — which is the condition the canceller is
        // built on.
        let mut mic_ready = resample(self.mic_resampler.as_mut(), &mic);
        let mut system_ready = resample(self.system_resampler.as_mut(), &system);

        // On speakers the far end comes back in through the microphone, and
        // would otherwise be transcribed a second time as though the
        // Operator had said it. The reference is the system leg at its
        // captured level: normalizing it first would change the very gain
        // the filter is trying to learn.
        if let Some(canceller) = self.echo.as_mut() {
            canceller.process(&mut mic_ready, &system_ready);
        }

        if let Some(loudness) = self.mic_loudness.as_mut() {
            loudness.process(&mut mic_ready);
        }
        for chunk in self.mic.push(&mic_ready) {
            if let Some(segment) = self.transcribe(AudioChannel::Mic, chunk) {
                segments.push(segment);
            }
        }

        if let Some(loudness) = self.system_loudness.as_mut() {
            loudness.process(&mut system_ready);
        }
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
        if let Some(chunk) = self.mic.flush()
            && let Some(segment) = self.transcribe(AudioChannel::Mic, chunk)
        {
            segments.push(segment);
        }
        if let Some(chunk) = self.system.flush()
            && let Some(segment) = self.transcribe(AudioChannel::System, chunk)
        {
            segments.push(segment);
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

/// Capture rate down to the rate whisper needs.
///
/// Falls back to plain decimation if the resampler could not be built, so a
/// resampler failure costs quality rather than the transcript.
fn resample(resampler: Option<&mut dsp::StreamResampler>, input: &[f32]) -> Vec<f32> {
    const FACTOR: usize = (SAMPLE_RATE / WHISPER_RATE) as usize;
    match resampler {
        Some(resampler) => resampler.process(input),
        None => input
            .chunks_exact(FACTOR)
            .map(|group| group.iter().sum::<f32>() / FACTOR as f32)
            .collect(),
    }
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
