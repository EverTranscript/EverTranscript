//! Drives one Meeting's capture: source → joiner → sink.
//!
//! The recorder is the "session owns the sink, streams are leaves" rule made
//! concrete. It is created when a Meeting starts and finalized when it stops;
//! nothing inside it can create or end a Meeting, which is structurally why
//! device churn cannot split one recording into two.

use std::path::PathBuf;

use anyhow::Result;
use evertranscript_protocol::AudioChannel;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tracing::debug;
use tracing::info;
use tracing::warn;

use super::AudioSource;
use super::CaptureClock;
use super::CaptureEvent;
use super::joiner::Joiner;
use super::joiner::StereoBlock;
use super::sink::CheckpointSink;
use super::supervisor::Action;
use super::supervisor::ChurnPolicy;
use crate::asr::pipeline::TranscribedSegment;
use crate::asr::pipeline::TranscriptionPipeline;

/// What a finished recording produced.
#[derive(Debug, Default)]
pub struct RecordingOutcome {
    /// The audio file, when one was written.
    pub audio_path: Option<PathBuf>,
    /// How much audio reached disk.
    pub seconds: f64,
    /// Legs that ended early, with why. Surfaced so a Meeting recorded with
    /// half its audio says so rather than looking complete.
    pub degraded: Vec<String>,
    /// How many Transcript segments this recording produced.
    pub segments: usize,
}

/// A running recording.
pub struct Recorder {
    stop: CancellationToken,
    finished: oneshot::Receiver<RecordingOutcome>,
    clock: CaptureClock,
}

impl Recorder {
    /// Starts recording into `audio_dir`, keyed by the Meeting's id8.
    ///
    /// `transcriber` is optional: with no model downloaded the Meeting still
    /// records audio, it just has no live captions (ADR-0019).
    pub fn start(
        mut source: Box<dyn AudioSource>,
        audio_dir: PathBuf,
        meeting_key: String,
        transcriber: Option<Box<dyn crate::asr::Transcriber>>,
        segments: Option<mpsc::Sender<TranscribedSegment>>,
    ) -> Result<Self> {
        let clock = CaptureClock::start();
        let (events_tx, events_rx) = mpsc::channel::<CaptureEvent>(256);
        source.start(clock.clone(), events_tx)?;

        let sink = CheckpointSink::new(&audio_dir, &meeting_key)?;
        let stop = CancellationToken::new();
        let (finished_tx, finished_rx) = oneshot::channel();

        tokio::spawn(run(
            source,
            events_rx,
            sink,
            stop.clone(),
            finished_tx,
            meeting_key,
            transcriber.map(TranscriptionPipeline::new),
            segments,
        ));

        Ok(Self {
            stop,
            finished: finished_rx,
            clock,
        })
    }

    /// Records audio only, with no transcription.
    pub fn start_without_transcription(
        source: Box<dyn AudioSource>,
        audio_dir: PathBuf,
        meeting_key: String,
    ) -> Result<Self> {
        Self::start(source, audio_dir, meeting_key, None, None)
    }

    pub fn clock(&self) -> &CaptureClock {
        &self.clock
    }

    /// Stops capture and waits for the audio to be finalized.
    pub async fn finish(self) -> RecordingOutcome {
        self.stop.cancel();
        self.finished.await.unwrap_or_default()
    }
}

#[allow(clippy::too_many_arguments)]
async fn run(
    mut source: Box<dyn AudioSource>,
    mut events: mpsc::Receiver<CaptureEvent>,
    mut sink: CheckpointSink,
    stop: CancellationToken,
    finished: oneshot::Sender<RecordingOutcome>,
    meeting_key: String,
    transcription: Option<TranscriptionPipeline>,
    segments_tx: Option<mpsc::Sender<TranscribedSegment>>,
) {
    let mut joiner = Joiner::new();
    let mut policy = ChurnPolicy::default();
    let mut degraded = Vec::new();
    let mut transcribed = 0usize;

    // Transcription gets its own thread. A whisper window decodes in
    // seconds while the capture channel holds about one second of frames,
    // so decoding inline stalls this drain long enough for CoreAudio to
    // start dropping frames — losing audio that was captured perfectly
    // well. ADR-0019 puts the recording first, and that is only true if
    // the recording never waits for the transcript.
    let transcription = transcription.map(|pipeline| Transcription::spawn(pipeline, segments_tx));

    // Stopping is two steps, not one. Breaking out of the loop the moment
    // the Operator says stop would abandon whatever capture has already
    // handed over and not yet been written — the last fraction of a second
    // of the meeting, which is to say the end of somebody's sentence. So a
    // stop first silences the source, then consumes what is already in the
    // queue, and only then finalizes.
    let mut draining = false;
    loop {
        let event = if draining {
            match events.try_recv() {
                Ok(event) => event,
                // Nothing left, and the source is stopped, so nothing more
                // is coming.
                Err(_) => break,
            }
        } else {
            tokio::select! {
                _ = stop.cancelled() => {
                    source.stop();
                    draining = true;
                    continue;
                }
                event = events.recv() => match event {
                    Some(event) => event,
                    // Every source ended. The Meeting does not: it stops when
                    // the Operator (or Auto-Record) says so, not when hardware
                    // runs out.
                    None => break,
                },
            }
        };

        match policy.decide(&event) {
            Action::Continue => {}
            Action::RestartLeg { channel, after, .. } => {
                debug!(?channel, ?after, "capture leg restarting");
                // The source owns its own reconnection; the timeline simply
                // shows the gap, which the joiner fills with silence.
            }
            Action::EndLeg { channel, reason } => {
                info!(?channel, reason, "capture leg ended; the Meeting continues");
                joiner.finish_leg(channel);
                degraded.push(format!("{}: {reason}", channel_name(channel)));
            }
        }

        if let CaptureEvent::Frame(frame) = &event {
            joiner.push(frame);
        }
        for block in joiner.drain() {
            // Audio to disk first: the recording must survive even if
            // transcription is slow or broken.
            if let Err(error) = sink.write(&block).await {
                warn!(%error, "writing audio failed");
            }
            if let Some(worker) = transcription.as_ref() {
                worker.offer(block);
            }
        }
    }

    source.stop();
    if let Some(block) = joiner.flush() {
        if let Err(error) = sink.write(&block).await {
            warn!(%error, "writing the final audio block failed");
        }
        if let Some(worker) = transcription.as_ref() {
            worker.offer(block);
        }
    }
    // Closing the queue is what tells the worker to flush its tail, so the
    // last sentence of the Meeting is not lost (story 5). Joining it can
    // take as long as one decode, which is why it does not run on a
    // runtime thread.
    if let Some(worker) = transcription {
        let (count, dropped) = worker.finish().await;
        transcribed = count;
        if dropped > 0 {
            warn!(dropped, "transcription fell behind; captions were lost");
            degraded.push(format!(
                "captions: {dropped} block(s) went untranscribed because transcription fell behind"
            ));
        }
    }

    let seconds = sink.seconds_written();
    let audio_path = match sink.finalize().await {
        Ok(path) => path,
        Err(error) => {
            warn!(meeting = meeting_key, %error, "could not finalize the audio");
            // Losing the audio is a degraded Meeting, not a failed one: the
            // record is the transcript (ADR-0019). But it has to be *said*,
            // or the Meeting is indistinguishable from one nobody spoke in.
            degraded.push(format!("audio file: {error:#}"));
            None
        }
    };
    if audio_path.is_none() && seconds > 0.0 && degraded.is_empty() {
        degraded.push(format!(
            "audio file: {seconds:.1}s was captured but no file was produced"
        ));
    }

    let _ = finished.send(RecordingOutcome {
        audio_path,
        seconds,
        degraded,
        segments: transcribed,
    });
}

/// Transcription, running beside capture rather than inside it.
struct Transcription {
    blocks: std::sync::mpsc::SyncSender<StereoBlock>,
    handle: std::thread::JoinHandle<usize>,
    /// Blocks the worker could not keep up with. Counted rather than
    /// ignored: a caption silently missing is indistinguishable from
    /// nobody having spoken.
    dropped: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl Transcription {
    fn spawn(
        mut pipeline: TranscriptionPipeline,
        segments_tx: Option<mpsc::Sender<TranscribedSegment>>,
    ) -> Self {
        // Roughly a minute of blocks. Deep enough to ride out a slow
        // decode, bounded so a hopelessly slow machine loses captions
        // rather than memory.
        let (blocks, incoming) = std::sync::mpsc::sync_channel::<StereoBlock>(4096);
        let dropped = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let handle = std::thread::spawn(move || {
            let mut count = 0usize;
            let send = |segments: Vec<TranscribedSegment>, count: &mut usize| {
                let Some(sender) = segments_tx.as_ref() else {
                    *count += segments.len();
                    return;
                };
                for segment in segments {
                    *count += 1;
                    if sender.blocking_send(segment).is_err() {
                        return;
                    }
                }
            };
            while let Ok(block) = incoming.recv() {
                let produced = pipeline.push(&block);
                send(produced, &mut count);
            }
            // The tail: whatever is still buffered when capture ends.
            send(pipeline.flush(), &mut count);
            count
        });

        Self {
            blocks,
            handle,
            dropped,
        }
    }

    /// Hands a block over, or counts it lost. Never blocks capture.
    fn offer(&self, block: StereoBlock) {
        if self.blocks.try_send(block).is_err() {
            self.dropped
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Closes the queue and waits for the worker's tail. Returns the
    /// segment count and how many blocks were never transcribed.
    async fn finish(self) -> (usize, usize) {
        let Self {
            blocks,
            handle,
            dropped,
        } = self;
        drop(blocks);
        let count = tokio::task::spawn_blocking(move || handle.join().unwrap_or(0))
            .await
            .unwrap_or(0);
        (count, dropped.load(std::sync::atomic::Ordering::Relaxed))
    }
}

fn channel_name(channel: AudioChannel) -> &'static str {
    match channel {
        AudioChannel::Mic => "microphone",
        AudioChannel::System => "system audio",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::fixture::FixtureSource;
    use crate::audio::fixture::Step;
    use crate::audio::sink::ffmpeg_available;

    async fn skip_without_ffmpeg() -> bool {
        if ffmpeg_available().await {
            return false;
        }
        eprintln!("skipping: ffmpeg is not available on this machine");
        true
    }

    #[tokio::test]
    async fn a_recording_produces_an_audio_file() {
        if skip_without_ffmpeg().await {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let (source, delivered) = FixtureSource::with_completion(vec![
            Step::audio(AudioChannel::Mic, 400, 0.4),
            Step::audio(AudioChannel::System, 400, -0.4),
        ]);
        let recorder = Recorder::start_without_transcription(
            Box::new(source),
            dir.path().to_path_buf(),
            "abcd1234".to_string(),
        )
        .expect("start");

        // Wait for the script rather than guessing at an interval.
        delivered.await.expect("the script should finish");
        let outcome = recorder.finish().await;

        let path = outcome
            .audio_path
            .unwrap_or_else(|| panic!("an audio file (degraded {:?})", outcome.degraded));
        assert!(path.exists());
        assert!(outcome.degraded.is_empty(), "a clean run is not degraded");
        assert!(
            outcome.seconds > 0.3,
            "roughly the scripted length, got {}",
            outcome.seconds
        );
    }

    #[tokio::test]
    async fn stopping_keeps_the_audio_that_capture_already_handed_over() {
        // The end of a meeting is where people say what they agreed to. A
        // stop that breaks out of the loop with frames still queued drops
        // exactly that, and does it silently — the file is simply shorter
        // than the meeting was. This scripts far more audio than the
        // recorder can consume promptly, so anything abandoned shows up.
        if skip_without_ffmpeg().await {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let mut script = Vec::new();
        for _ in 0..50 {
            script.push(Step::audio(AudioChannel::Mic, 100, 0.4));
            script.push(Step::audio(AudioChannel::System, 100, -0.4));
        }
        let (source, delivered) = FixtureSource::with_completion(script);
        let recorder = Recorder::start_without_transcription(
            Box::new(source),
            dir.path().to_path_buf(),
            "abcd1234".to_string(),
        )
        .expect("start");

        // Stop the instant the script is delivered, which is the worst case:
        // the queue is as full as it will ever be.
        delivered.await.expect("the script should finish");
        let outcome = recorder.finish().await;

        assert!(
            outcome.audio_path.is_some(),
            "an audio file (degraded {:?})",
            outcome.degraded
        );
        assert!(
            outcome.seconds > 4.5,
            "5s was captured, so roughly 5s must reach the file — got {}",
            outcome.seconds
        );
    }

    #[tokio::test]
    async fn losing_system_audio_does_not_stop_the_recording() {
        if skip_without_ffmpeg().await {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let (source, delivered) = FixtureSource::with_completion(vec![
            Step::audio(AudioChannel::Mic, 200, 0.5),
            Step::Unavailable {
                channel: AudioChannel::System,
                reason: "system audio capture unavailable".to_string(),
            },
            Step::audio(AudioChannel::Mic, 200, 0.5),
        ]);
        let recorder = Recorder::start_without_transcription(
            Box::new(source),
            dir.path().to_path_buf(),
            "abcd1234".to_string(),
        )
        .expect("start");

        delivered.await.expect("the script should finish");
        let outcome = recorder.finish().await;

        assert!(
            outcome.audio_path.is_some(),
            "the microphone recording must survive losing system audio \
             (captured {}s, degraded {:?})",
            outcome.seconds,
            outcome.degraded
        );
        assert_eq!(outcome.degraded.len(), 1);
        assert!(outcome.degraded[0].contains("system audio"));
    }

    #[tokio::test]
    async fn a_device_swap_keeps_one_recording_rather_than_splitting_it() {
        if skip_without_ffmpeg().await {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let (source, delivered) = FixtureSource::with_completion(vec![
            Step::audio(AudioChannel::Mic, 200, 0.5),
            Step::audio(AudioChannel::System, 200, -0.5),
            // AirPods disconnect: the device changes and capture pauses.
            Step::DeviceChange {
                channel: AudioChannel::Mic,
            },
            Step::Gap { ms: 300 },
            // …and comes back on the built-in microphone.
            Step::audio(AudioChannel::Mic, 200, 0.5),
            Step::audio(AudioChannel::System, 500, -0.5),
        ]);
        let recorder = Recorder::start_without_transcription(
            Box::new(source),
            dir.path().to_path_buf(),
            "abcd1234".to_string(),
        )
        .expect("start");

        delivered.await.expect("the script should finish");
        let outcome = recorder.finish().await;

        assert!(outcome.audio_path.is_some(), "one file, not two");
        assert!(
            outcome.degraded.is_empty(),
            "a device swap is housekeeping, not degradation: {:?}",
            outcome.degraded
        );
        // 700 ms of timeline: 200 recorded + 300 gap + 200 recorded. The gap
        // is silence rather than missing time, so audio stays aligned with
        // the transcript across the swap.
        assert!(
            outcome.seconds >= 0.65,
            "the outage must be represented, not skipped; got {}s",
            outcome.seconds
        );
    }
}
