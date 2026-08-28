//! Deciding where an utterance starts and stops.
//!
//! whisper.cpp is not a streaming model: it transcribes a buffer. Live
//! captions therefore need something to decide *which* buffer, and that
//! decision is most of the quality. Chunk too eagerly and words are cut in
//! half; chunk too lazily and captions lag by half a minute.
//!
//! The chunker is duration-adaptive, following anarlog's shipped envelope:
//! the threshold for calling a pause "the end" starts strict and relaxes as
//! a chunk gets longer, so short utterances need a convincing silence to be
//! closed while a chunk approaching the target ends at the first natural
//! breath.
//!
//! The speech detector behind it is an energy gate rather than a neural VAD.
//! That is a deliberate de-risking, not a shortcut: Granola ships an energy
//! gate on its live path, so a neural VAD is not a prerequisite for working
//! captions. The trait below is the seam Silero drops into later.

/// Decides whether a frame contains speech. `probability` is in `[0, 1]`.
pub trait SpeechDetector: Send {
    fn probability(&mut self, frame: &[f32]) -> f32;
    fn reset(&mut self) {}
}

/// Energy gate: loud enough, and not obviously a hum or a hiss.
///
/// Two features rather than one, because level alone treats a fan as speech
/// and a quiet talker as silence. Zero-crossing rate separates voiced speech
/// (low) from hiss (high) and rumble (very low).
pub struct EnergyDetector {
    /// RMS below this is silence regardless of anything else.
    floor: f32,
    /// Adaptive noise estimate, so a noisy room raises the bar instead of
    /// transcribing its own air conditioning.
    noise_floor: f32,
}

impl Default for EnergyDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl EnergyDetector {
    pub fn new() -> Self {
        Self {
            floor: 0.006,
            // Starts at the fixed floor rather than at the first frame's
            // level. Seeding from the first frame means a meeting joined
            // mid-sentence sets its noise floor to *speech* and then hears
            // silence for the rest of the call.
            noise_floor: 0.006,
        }
    }
}

impl SpeechDetector for EnergyDetector {
    fn probability(&mut self, frame: &[f32]) -> f32 {
        if frame.is_empty() {
            return 0.0;
        }
        let rms = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt();

        // Track the quietest recent level as the room's noise floor. It
        // falls quickly toward a new quiet level and rises very slowly, so a
        // long loud passage cannot desensitise the gate to the quiet talker
        // who speaks after it.
        if rms < self.noise_floor {
            self.noise_floor = self.noise_floor * 0.9 + rms * 0.1;
        } else {
            self.noise_floor = self.noise_floor * 0.9995 + rms * 0.0005;
        }

        let threshold = self.floor.max(self.noise_floor * 2.5);
        if rms < threshold {
            return 0.0;
        }

        let crossings = frame
            .windows(2)
            .filter(|pair| (pair[0] < 0.0) != (pair[1] < 0.0))
            .count() as f32
            / frame.len() as f32;
        // Voiced speech sits well below 0.35 crossings per sample; hiss and
        // clicks sit above it.
        if crossings > 0.35 {
            return 0.2;
        }

        ((rms / threshold - 1.0) / 2.0).clamp(0.0, 1.0).max(0.55)
    }

    fn reset(&mut self) {
        self.noise_floor = self.floor;
    }
}

/// Tuning for the chunker. Defaults follow anarlog's shipped envelope.
#[derive(Debug, Clone, Copy)]
pub struct ChunkPolicy {
    /// Probability above which a frame starts (or continues) speech.
    pub positive_threshold: f32,
    /// Probability below which a frame counts as silence once speech is
    /// under way. Lower than the positive threshold so a brief dip inside a
    /// word does not end the utterance.
    pub negative_threshold: f32,
    /// The strict end of the adaptive range, used while a chunk is short.
    pub max_negative_threshold: f32,
    /// How long a pause must last before a chunk is closed.
    pub redemption_ms: u64,
    /// Audio kept before speech starts, so the first consonant survives.
    pub pre_speech_pad_ms: u64,
    /// Chunks shorter than this are held back rather than transcribed.
    pub min_chunk_ms: u64,
    /// Where the adaptive threshold finishes relaxing: past this, almost any
    /// pause ends the chunk.
    pub target_chunk_ms: u64,
    /// Hard cap. whisper's window is 30s; staying under it avoids the model
    /// silently truncating.
    pub max_chunk_ms: u64,
    /// Utterances shorter than this are dropped as clicks and lip noise —
    /// the pre-decode gate that stops silence becoming a hallucination.
    pub min_speech_ms: u64,
}

impl Default for ChunkPolicy {
    fn default() -> Self {
        Self {
            positive_threshold: 0.5,
            negative_threshold: 0.35,
            max_negative_threshold: 0.80,
            redemption_ms: 600,
            pre_speech_pad_ms: 300,
            min_chunk_ms: 3_000,
            target_chunk_ms: 20_000,
            max_chunk_ms: 25_000,
            min_speech_ms: 90,
        }
    }
}

impl ChunkPolicy {
    /// The silence threshold for a chunk that has been running `elapsed_ms`.
    ///
    /// Strict early, relaxed later: a two-second chunk needs a convincing
    /// pause to be closed, while one approaching the target ends at the
    /// first natural breath. This is what makes chunks land on sentence
    /// boundaries instead of mid-clause.
    pub fn negative_threshold_at(&self, elapsed_ms: u64) -> f32 {
        if elapsed_ms >= self.target_chunk_ms {
            return self.negative_threshold;
        }
        if elapsed_ms <= self.min_chunk_ms {
            return self.max_negative_threshold;
        }
        let span = (self.target_chunk_ms - self.min_chunk_ms) as f32;
        let progress = (elapsed_ms - self.min_chunk_ms) as f32 / span;
        self.max_negative_threshold
            - progress * (self.max_negative_threshold - self.negative_threshold)
    }
}

/// A span of audio the chunker decided is worth transcribing.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeechChunk {
    /// Offset of the first sample, on the capture clock.
    pub start_ms: u64,
    pub end_ms: u64,
    pub samples: Vec<f32>,
    /// True when the chunk was cut by the hard cap rather than by a pause,
    /// so the caller knows the sentence probably continues.
    pub forced: bool,
}

impl SpeechChunk {
    pub fn duration_ms(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }
}

/// Turns a stream of audio into transcribable chunks.
pub struct Chunker {
    policy: ChunkPolicy,
    detector: Box<dyn SpeechDetector>,
    rate: u32,
    /// Audio held for the current chunk, plus the pre-speech pad.
    buffer: Vec<f32>,
    /// Offset of `buffer[0]`.
    buffer_start_ms: u64,
    /// Total audio consumed, in samples.
    consumed: u64,
    in_speech: bool,
    /// How long the current silence has run.
    silence_ms: u64,
    /// How much speech this chunk contains.
    speech_ms: u64,
    frame_samples: usize,
    /// Audio left over from the last push: shorter than one frame, so it
    /// cannot be judged yet. Carried rather than dropped, because live
    /// capture delivers blocks far smaller than a frame.
    pending: Vec<f32>,
}

impl Chunker {
    pub fn new(rate: u32, policy: ChunkPolicy, detector: Box<dyn SpeechDetector>) -> Self {
        // 30 ms frames: fine enough to place a boundary, coarse enough that
        // the energy estimate is stable.
        let frame_samples = (rate as usize / 1000) * 30;
        Self {
            policy,
            detector,
            rate,
            buffer: Vec::new(),
            buffer_start_ms: 0,
            consumed: 0,
            in_speech: false,
            silence_ms: 0,
            speech_ms: 0,
            frame_samples,
            pending: Vec::new(),
        }
    }

    pub fn with_defaults(rate: u32) -> Self {
        Self::new(
            rate,
            ChunkPolicy::default(),
            Box::new(EnergyDetector::new()),
        )
    }

    fn samples_to_ms(&self, samples: usize) -> u64 {
        samples as u64 * 1000 / self.rate as u64
    }

    fn ms_to_samples(&self, ms: u64) -> usize {
        (ms * self.rate as u64 / 1000) as usize
    }

    /// Feeds audio and returns any chunks that closed.
    pub fn push(&mut self, samples: &[f32]) -> Vec<SpeechChunk> {
        let mut chunks = Vec::new();
        let mut offset = 0;

        // A CoreAudio callback is a fraction of a frame, so judging only
        // what arrived in this call discards all of it, every time. The
        // remainder joins the next block instead.
        let mut pending = std::mem::take(&mut self.pending);
        pending.extend_from_slice(samples);

        while offset + self.frame_samples <= pending.len() {
            let frame = &pending[offset..offset + self.frame_samples];
            offset += self.frame_samples;

            let frame_ms = self.samples_to_ms(self.frame_samples);
            let probability = self.detector.probability(frame);
            let elapsed = self.samples_to_ms(self.buffer.len());
            let negative = self.policy.negative_threshold_at(elapsed);

            if self.buffer.is_empty() {
                self.buffer_start_ms = self.samples_to_ms(self.consumed as usize);
            }
            self.buffer.extend_from_slice(frame);
            self.consumed += self.frame_samples as u64;

            if probability >= self.policy.positive_threshold {
                self.in_speech = true;
                self.silence_ms = 0;
                self.speech_ms += frame_ms;
            } else if self.in_speech && probability < negative {
                self.silence_ms += frame_ms;
            }

            if !self.in_speech {
                // Keep only the pre-speech pad so the first consonant of the
                // next utterance is not clipped.
                let keep = self.ms_to_samples(self.policy.pre_speech_pad_ms);
                if self.buffer.len() > keep {
                    let drop = self.buffer.len() - keep;
                    self.buffer.drain(..drop);
                    self.buffer_start_ms += self.samples_to_ms(drop);
                }
                continue;
            }

            let chunk_ms = self.samples_to_ms(self.buffer.len());
            let ended_on_a_pause = self.silence_ms >= self.policy.redemption_ms
                && chunk_ms >= self.policy.min_chunk_ms;
            let hit_the_cap = chunk_ms >= self.policy.max_chunk_ms;

            if (ended_on_a_pause || hit_the_cap)
                && let Some(chunk) = self.close(hit_the_cap)
            {
                chunks.push(chunk);
            }
        }
        pending.drain(..offset);
        self.pending = pending;
        chunks
    }

    /// Closes the current chunk, if it holds enough speech to be worth
    /// transcribing.
    fn close(&mut self, forced: bool) -> Option<SpeechChunk> {
        let samples = std::mem::take(&mut self.buffer);
        let start_ms = self.buffer_start_ms;
        let end_ms = start_ms + self.samples_to_ms(samples.len());
        let speech_ms = self.speech_ms;

        self.buffer_start_ms = end_ms;
        self.in_speech = false;
        self.silence_ms = 0;
        self.speech_ms = 0;

        // Too little actual speech: transcribing this is how "thank you for
        // watching" gets into an immutable record.
        if speech_ms < self.policy.min_speech_ms || samples.is_empty() {
            return None;
        }
        Some(SpeechChunk {
            start_ms,
            end_ms,
            samples,
            forced,
        })
    }

    /// Closes whatever is buffered at the end of a Meeting, so the last
    /// sentence is not lost when the Operator hits stop.
    pub fn flush(&mut self) -> Option<SpeechChunk> {
        // The last partial frame belongs to the last sentence (story 5).
        if !self.pending.is_empty() {
            let tail = std::mem::take(&mut self.pending);
            if self.buffer.is_empty() {
                self.buffer_start_ms = self.samples_to_ms(self.consumed as usize);
            }
            self.consumed += tail.len() as u64;
            self.buffer.extend_from_slice(&tail);
        }
        if self.buffer.is_empty() {
            return None;
        }
        self.close(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 16_000;

    fn speech(ms: u64) -> Vec<f32> {
        // A 200 Hz tone: loud, low zero-crossing — what the gate calls voice.
        let count = (RATE as u64 * ms / 1000) as usize;
        (0..count)
            .map(|index| (index as f32 / RATE as f32 * 200.0 * std::f32::consts::TAU).sin() * 0.3)
            .collect()
    }

    fn silence(ms: u64) -> Vec<f32> {
        vec![0.0; (RATE as u64 * ms / 1000) as usize]
    }

    #[test]
    fn the_pause_threshold_relaxes_as_a_chunk_grows() {
        let policy = ChunkPolicy::default();
        let early = policy.negative_threshold_at(1_000);
        let middle = policy.negative_threshold_at(10_000);
        let late = policy.negative_threshold_at(20_000);

        assert_eq!(
            early, policy.max_negative_threshold,
            "short chunks are strict"
        );
        assert!(
            middle < early && middle > late,
            "the threshold should relax monotonically: {early} -> {middle} -> {late}"
        );
        assert_eq!(late, policy.negative_threshold, "long chunks end easily");
    }

    #[test]
    fn speech_followed_by_a_pause_produces_one_chunk() {
        let mut chunker = Chunker::with_defaults(RATE);
        let mut chunks = chunker.push(&speech(4_000));
        chunks.extend(chunker.push(&silence(1_000)));

        assert_eq!(chunks.len(), 1, "one utterance, one chunk");
        let chunk = &chunks[0];
        assert!(
            chunk.duration_ms() >= 3_000,
            "the chunk should hold the speech, got {}ms",
            chunk.duration_ms()
        );
        assert!(!chunk.forced, "it ended on a pause, not on the cap");
    }

    #[test]
    fn a_brief_dip_inside_a_word_does_not_split_the_utterance() {
        let mut chunker = Chunker::with_defaults(RATE);
        let mut chunks = chunker.push(&speech(2_000));
        // 200 ms is a plosive, not the end of a sentence.
        chunks.extend(chunker.push(&silence(200)));
        chunks.extend(chunker.push(&speech(2_000)));
        chunks.extend(chunker.push(&silence(1_000)));

        assert_eq!(
            chunks.len(),
            1,
            "a short pause must not cut the utterance in two"
        );
    }

    #[test]
    fn a_monologue_is_cut_at_the_cap_and_marked_forced() {
        let mut chunker = Chunker::with_defaults(RATE);
        // 40 seconds without a pause: well past whisper's 30s window.
        let chunks = chunker.push(&speech(40_000));

        assert!(!chunks.is_empty(), "the cap must produce chunks");
        for chunk in &chunks {
            assert!(
                chunk.duration_ms() <= ChunkPolicy::default().max_chunk_ms + 100,
                "no chunk may exceed the cap, got {}ms",
                chunk.duration_ms()
            );
        }
        assert!(
            chunks.iter().any(|chunk| chunk.forced),
            "a chunk cut by the cap must say so, since the sentence continues"
        );
    }

    #[test]
    fn silence_alone_never_produces_a_chunk() {
        // The most important case: given nothing, produce nothing. Anything
        // that reaches whisper here becomes a hallucination in an immutable
        // record.
        let mut chunker = Chunker::with_defaults(RATE);
        let chunks = chunker.push(&silence(30_000));
        assert!(chunks.is_empty(), "silence must not be transcribed");
        assert!(
            chunker.flush().is_none(),
            "and must not flush a chunk either"
        );
    }

    #[test]
    fn a_click_is_too_short_to_transcribe() {
        let mut chunker = Chunker::with_defaults(RATE);
        let mut chunks = chunker.push(&silence(500));
        chunks.extend(chunker.push(&speech(60))); // shorter than min_speech_ms
        chunks.extend(chunker.push(&silence(2_000)));
        assert!(
            chunks.is_empty(),
            "a 60 ms blip is lip noise, not an utterance"
        );
    }

    #[test]
    fn the_last_sentence_survives_a_stop() {
        // Story 5: hitting stop must not lose what was just said.
        let mut chunker = Chunker::with_defaults(RATE);
        let chunks = chunker.push(&speech(1_500));
        assert!(chunks.is_empty(), "still mid-utterance");

        let flushed = chunker.flush().expect("the tail must be recoverable");
        assert!(flushed.duration_ms() >= 1_000);
    }

    #[test]
    fn chunk_offsets_are_contiguous_on_the_capture_clock() {
        let mut chunker = Chunker::with_defaults(RATE);
        let mut chunks = Vec::new();
        for _ in 0..3 {
            chunks.extend(chunker.push(&speech(4_000)));
            chunks.extend(chunker.push(&silence(1_000)));
        }
        assert!(chunks.len() >= 2, "expected several chunks");
        for pair in chunks.windows(2) {
            assert!(
                pair[1].start_ms >= pair[0].end_ms,
                "chunks must not overlap: {} then {}",
                pair[0].end_ms,
                pair[1].start_ms
            );
        }
    }

    #[test]
    fn the_energy_gate_separates_speech_from_room_tone() {
        let mut detector = EnergyDetector::new();
        let quiet: Vec<f32> = (0..480)
            .map(|i| ((i as f32) * 0.01).sin() * 0.001)
            .collect();
        for _ in 0..20 {
            detector.probability(&quiet);
        }
        assert!(
            detector.probability(&quiet) < 0.5,
            "room tone is not speech"
        );

        let loud = speech(30);
        assert!(
            detector.probability(&loud[..480]) >= 0.5,
            "a clear voiced tone is speech"
        );
    }

    /// Live capture delivers audio in small CoreAudio callbacks, not in
    /// whole files. The same speech must chunk identically however it is
    /// sliced on the way in.
    #[test]
    fn speech_chunks_the_same_when_delivered_in_small_blocks() {
        let audio = {
            let mut audio = speech(2_000);
            audio.extend(silence(1_500));
            audio
        };

        let mut whole = Chunker::with_defaults(RATE);
        let in_one_call = whole.push(&audio).len();

        // 10 ms at 16 kHz is 160 samples — a third of one 30 ms frame,
        // which is what a resampled CoreAudio callback actually looks like.
        let mut streamed = Chunker::with_defaults(RATE);
        let mut in_small_blocks = 0;
        for block in audio.chunks(160) {
            in_small_blocks += streamed.push(block).len();
        }

        assert_eq!(
            in_small_blocks, in_one_call,
            "{in_one_call} chunk(s) when pushed whole but {in_small_blocks} when \
             pushed in 160-sample blocks: audio shorter than one frame is being dropped"
        );
    }
}
