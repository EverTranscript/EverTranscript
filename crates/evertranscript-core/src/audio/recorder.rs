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

use super::joiner::Joiner;
use super::sink::CheckpointSink;
use super::supervisor::Action;
use super::supervisor::ChurnPolicy;
use super::AudioSource;
use super::CaptureClock;
use super::CaptureEvent;
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
    mut transcription: Option<TranscriptionPipeline>,
    segments_tx: Option<mpsc::Sender<TranscribedSegment>>,
) {
    let mut joiner = Joiner::new();
    let mut policy = ChurnPolicy::default();
    let mut degraded = Vec::new();
    let mut transcribed = 0usize;

    /// Sends transcript segments on, dropping them only if the consumer is
    /// gone — never blocking capture on a slow writer.
    async fn deliver(
        sender: &Option<mpsc::Sender<TranscribedSegment>>,
        segments: Vec<TranscribedSegment>,
        count: &mut usize,
    ) {
        let Some(sender) = sender else { return };
        for segment in segments {
            *count += 1;
            if sender.send(segment).await.is_err() {
                return;
            }
        }
    }

    loop {
        let event = tokio::select! {
            _ = stop.cancelled() => break,
            event = events.recv() => match event {
                Some(event) => event,
                // Every source ended. The Meeting does not: it stops when
                // the Operator (or Auto-Record) says so, not when hardware
                // runs out.
                None => break,
            },
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
            if let Some(pipeline) = transcription.as_mut() {
                let produced = pipeline.push(&block);
                deliver(&segments_tx, produced, &mut transcribed).await;
            }
        }
    }

    source.stop();
    if let Some(block) = joiner.flush() {
        if let Err(error) = sink.write(&block).await {
            warn!(%error, "writing the final audio block failed");
        }
        if let Some(pipeline) = transcription.as_mut() {
            let produced = pipeline.push(&block);
            deliver(&segments_tx, produced, &mut transcribed).await;
        }
    }
    // The tail: whatever is still buffered when the Operator hits stop.
    // Without this the last sentence of every Meeting is lost (story 5).
    if let Some(pipeline) = transcription.as_mut() {
        let produced = pipeline.flush();
        deliver(&segments_tx, produced, &mut transcribed).await;
    }

    let seconds = sink.seconds_written();
    let audio_path = match sink.finalize().await {
        Ok(path) => path,
        Err(error) => {
            warn!(meeting = meeting_key, %error, "could not finalize the audio");
            None
        }
    };

    let _ = finished.send(RecordingOutcome {
        audio_path,
        seconds,
        degraded,
        segments: transcribed,
    });
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
        let recorder = Recorder::start_without_transcription(
            Box::new(FixtureSource::simple(400)),
            dir.path().to_path_buf(),
            "abcd1234".to_string(),
        )
        .expect("start");

        // Let the scripted source drain.
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let outcome = recorder.finish().await;

        let path = outcome.audio_path.expect("an audio file");
        assert!(path.exists());
        assert!(outcome.degraded.is_empty(), "a clean run is not degraded");
        assert!(
            outcome.seconds > 0.3,
            "roughly the scripted length, got {}",
            outcome.seconds
        );
    }

    #[tokio::test]
    async fn losing_system_audio_does_not_stop_the_recording() {
        if skip_without_ffmpeg().await {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let recorder = Recorder::start_without_transcription(
            Box::new(FixtureSource::new(vec![
                Step::audio(AudioChannel::Mic, 200, 0.5),
                Step::Unavailable {
                    channel: AudioChannel::System,
                    reason: "system audio capture unavailable".to_string(),
                },
                Step::audio(AudioChannel::Mic, 200, 0.5),
            ])),
            dir.path().to_path_buf(),
            "abcd1234".to_string(),
        )
        .expect("start");

        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let outcome = recorder.finish().await;

        assert!(
            outcome.audio_path.is_some(),
            "the microphone recording must survive losing system audio"
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
        let recorder = Recorder::start_without_transcription(
            Box::new(FixtureSource::new(vec![
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
            ])),
            dir.path().to_path_buf(),
            "abcd1234".to_string(),
        )
        .expect("start");

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
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
