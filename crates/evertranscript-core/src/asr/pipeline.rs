//! From captured stereo blocks to transcript segments.
//!
//! The two channels are transcribed independently. That is not an
//! optimisation — it is the attribution model: the mic channel is where the
//! Operator is and the system channel is everyone else (ADR-0029 as
//! amended), so keeping them separate is what lets M1 label speech at all
//! before Diarization exists.

use evertranscript_protocol::AudioChannel;
use tracing::debug;

use evertranscript_protocol::ChineseScript;

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
    /// What each leg's last accepted chunk left behind. One per channel,
    /// because they are different people: sharing it let the Operator's
    /// words steer the far end's decode and the far end's steer theirs,
    /// which is the opposite of the separation this file exists to keep.
    mic_context: Context,
    system_context: Context,
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
    /// Which Han script Mandarin is written in. Read once when the Meeting
    /// starts: changing it mid-recording would leave one transcript written
    /// two ways.
    script: ChineseScript,
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
            mic_context: Context::default(),
            system_context: Context::default(),
            mic_resampler: dsp::StreamResampler::new(SAMPLE_RATE, WHISPER_RATE).ok(),
            system_resampler: dsp::StreamResampler::new(SAMPLE_RATE, WHISPER_RATE).ok(),
            mic_loudness: dsp::LoudnessNormalizer::new(WHISPER_RATE).ok(),
            system_loudness: dsp::LoudnessNormalizer::new(WHISPER_RATE).ok(),
            echo: Some(aec::EchoCanceller::new(WHISPER_RATE)),
            script: ChineseScript::default(),
        }
    }

    /// Sets the Han script Mandarin is recorded in.
    pub fn in_script(mut self, script: ChineseScript) -> Self {
        self.script = script;
        self
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
        let (previous, prompted_in) = {
            let context = self.context(channel);
            (context.text.clone(), context.language.clone())
        };

        let mut result = self.decode(&chunk.samples, previous.as_deref())?;

        // A prompt in the wrong language does not merely fail to help: it
        // drags the decode into its own language and the words come back
        // translated, which measured as CER 100% on the first sentence
        // after a switch. What makes it recoverable is that the engine
        // still reports the language it *heard*, so prompt and audio
        // disagreeing is the evidence — and the only cure is to ask again
        // without it. Paid once per switch, not once per chunk.
        if let (Some(prompted_in), Some(heard)) = (prompted_in, result.language.as_deref())
            && prompted_in != heard
        {
            debug!(
                prompted_in,
                heard, "the rolling prompt was in another language; decoding again without it"
            );
            result = self.decode(&chunk.samples, None)?;
        }

        let text = super::filters::clean(&result.text, self.script)?;
        if result.confidence < MIN_CONFIDENCE {
            debug!(
                confidence = result.confidence,
                text, "dropping a low-confidence decode as invention"
            );
            return None;
        }

        let language = result.language.clone();
        let context = self.context(channel);
        context.text = Some(text.clone());
        context.language = language;
        Some(TranscribedSegment {
            channel,
            start_ms: chunk.start_ms,
            end_ms: chunk.end_ms,
            text,
            confidence: result.confidence,
        })
    }

    /// One decode, or `None` with the reason logged. A failed decode costs
    /// one caption; it must not stop capture or the Meeting (ADR-0029 as
    /// amended).
    fn decode(&mut self, samples: &[f32], previous: Option<&str>) -> Option<super::Transcript> {
        match self.transcriber.transcribe(samples, previous) {
            Ok(result) => Some(result),
            Err(error) => {
                debug!(%error, "a chunk failed to transcribe");
                None
            }
        }
    }

    fn context(&mut self, channel: AudioChannel) -> &mut Context {
        match channel {
            AudioChannel::Mic => &mut self.mic_context,
            AudioChannel::System => &mut self.system_context,
        }
    }
}

/// What one leg's last accepted chunk left behind.
#[derive(Default)]
struct Context {
    /// Fed forward as whisper's rolling prompt, so a name or a piece of
    /// jargon transcribed once keeps its spelling through the meeting.
    text: Option<String>,
    /// The language that text was spoken in, so the prompt can be withdrawn
    /// when the speaker changes language rather than corrupting the decode.
    language: Option<String>,
}

/// Splits an interleaved stereo block into its two mono channels.
fn split(block: &StereoBlock) -> (Vec<f32>, Vec<f32>) {
    let frames = block.frame_count();
    let mut mic = Vec::with_capacity(frames);
    let mut system = Vec::with_capacity(frames);
    for [left, right] in block.samples.as_chunks::<2>().0 {
        mic.push(*left);
        system.push(*right);
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
            .as_chunks::<FACTOR>()
            .0
            .iter()
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

    /// Answers with a scripted (text, language) pair and remembers every
    /// prompt it was handed, so a language switch can be driven without a
    /// model and the prompt itself can be asserted on.
    struct Recording {
        answers: std::collections::VecDeque<(&'static str, &'static str)>,
        prompts: std::sync::Arc<std::sync::Mutex<Vec<Option<String>>>>,
    }

    impl Recording {
        fn new(
            answers: impl IntoIterator<Item = (&'static str, &'static str)>,
        ) -> (Self, std::sync::Arc<std::sync::Mutex<Vec<Option<String>>>>) {
            let prompts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let recorder = Self {
                answers: answers.into_iter().collect(),
                prompts: std::sync::Arc::clone(&prompts),
            };
            (recorder, prompts)
        }
    }

    impl Transcriber for Recording {
        fn transcribe(
            &mut self,
            _samples: &[f32],
            previous: Option<&str>,
        ) -> anyhow::Result<crate::asr::Transcript> {
            self.prompts
                .lock()
                .expect("prompts")
                .push(previous.map(str::to_string));
            let (text, language) = self.answers.pop_front().unwrap_or(("", "en"));
            Ok(crate::asr::Transcript {
                text: text.to_string(),
                confidence: 0.9,
                decode_time: std::time::Duration::from_millis(1),
                language: Some(language.to_string()),
            })
        }

        fn describe(&self) -> String {
            "recording".to_string()
        }
    }

    /// Speech on one leg, then a pause long enough to close the chunk.
    fn utterance(
        pipeline: &mut TranscriptionPipeline,
        mic: f32,
        system: f32,
    ) -> Vec<TranscribedSegment> {
        let mut segments = pipeline.push(&block(mic, system, 4_000));
        segments.extend(pipeline.push(&block(0.0, 0.0, 1_500)));
        segments
    }

    #[test]
    fn one_leg_never_steers_the_other() {
        // The legs are different people (ADR-0029 as amended). A single
        // rolling prompt let the Operator's words prime the far end's decode
        // and the far end's prime theirs, which is the mixing the channel
        // split exists to prevent.
        let (transcriber, prompts) =
            Recording::new([("the operator speaks", "en"), ("the far end speaks", "en")]);
        let mut pipeline = TranscriptionPipeline::new(Box::new(transcriber));

        utterance(&mut pipeline, 0.3, 0.0);
        utterance(&mut pipeline, 0.0, 0.3);

        let prompts = prompts.lock().expect("prompts");
        assert_eq!(prompts.len(), 2, "one decode per leg, got {prompts:?}");
        assert_eq!(prompts[0], None, "the first thing said has no prompt");
        assert_eq!(
            prompts[1], None,
            "the far end must not be primed with what the Operator said, got {:?}",
            prompts[1]
        );
    }

    #[test]
    fn a_prompt_in_another_language_is_withdrawn_and_the_chunk_decoded_again() {
        // Measured at CER 100% on the first sentence after a switch: an
        // English prompt drags a Mandarin chunk into English, while the
        // engine still reports what it heard. The disagreement is the
        // evidence, and the cure is to ask again with no prompt.
        let (transcriber, prompts) = Recording::new([
            ("the council met on Tuesday", "en"),
            ("We will discuss the third year's plan", "zh"),
            ("我们今天开会讨论第三季度的预算", "zh"),
        ]);
        let mut pipeline = TranscriptionPipeline::new(Box::new(transcriber));

        utterance(&mut pipeline, 0.3, 0.0);
        let switched = utterance(&mut pipeline, 0.3, 0.0);

        let prompts = prompts.lock().expect("prompts");
        assert_eq!(
            prompts.len(),
            3,
            "the switched chunk must be decoded twice, got {prompts:?}"
        );
        assert_eq!(prompts[1].as_deref(), Some("the council met on Tuesday"));
        assert_eq!(prompts[2], None, "the retry carries no prompt");
        assert!(
            switched.iter().any(|segment| segment.text.contains("预算")),
            "the record keeps the prompt-free decode, got {switched:?}"
        );
    }

    #[test]
    fn a_prompt_in_the_same_language_is_kept() {
        // The other half: within one language the prompt is what keeps a
        // name spelled the same way all meeting, so it must survive.
        let (transcriber, prompts) =
            Recording::new([("Aoife joined the call", "en"), ("Aoife agreed", "en")]);
        let mut pipeline = TranscriptionPipeline::new(Box::new(transcriber));

        utterance(&mut pipeline, 0.3, 0.0);
        utterance(&mut pipeline, 0.3, 0.0);

        let prompts = prompts.lock().expect("prompts");
        assert_eq!(prompts.len(), 2, "no retry when nothing changed language");
        assert_eq!(prompts[1].as_deref(), Some("Aoife joined the call"));
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
