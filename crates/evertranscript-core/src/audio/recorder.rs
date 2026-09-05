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
use super::StereoBlock;
use super::joiner::Joiner;
use super::sink::AudioSink;
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

/// Everything a recording needs in order to caption itself.
///
/// The three arrived as separate arguments and always travelled together,
/// because none of them means anything without the others: an engine with
/// nowhere to send segments produces nothing anyone can read, and a script
/// is a decision about text that is only taken if there is text. Together
/// they are one question — is this Meeting being captioned? — and `Option`
/// now asks it once instead of three times, which is what stops a recorder
/// that never transcribes from having to name a script.
pub struct Captions {
    pub transcriber: Box<dyn crate::asr::Transcriber>,
    pub segments: mpsc::Sender<TranscribedSegment>,
    /// Which Han script Mandarin is written in (DECISIONS Q11).
    pub script: evertranscript_protocol::ChineseScript,
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
    /// `captions` is optional: with no model downloaded the Meeting still
    /// records audio, it just has no live captions (ADR-0019).
    pub fn start(
        mut source: Box<dyn AudioSource>,
        audio_dir: PathBuf,
        meeting_key: String,
        captions: Option<Captions>,
    ) -> Result<Self> {
        let clock = CaptureClock::start();
        let (events_tx, events_rx) = mpsc::channel::<CaptureEvent>(256);
        source.start(clock.clone(), events_tx)?;

        let sink = AudioSink::new(&audio_dir, &meeting_key)?;
        let stop = CancellationToken::new();
        let (finished_tx, finished_rx) = oneshot::channel();

        tokio::spawn(run(
            source,
            events_rx,
            sink,
            stop.clone(),
            finished_tx,
            meeting_key,
            captions,
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
        Self::start(source, audio_dir, meeting_key, None)
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

async fn run(
    mut source: Box<dyn AudioSource>,
    mut events: mpsc::Receiver<CaptureEvent>,
    mut sink: AudioSink,
    stop: CancellationToken,
    finished: oneshot::Sender<RecordingOutcome>,
    meeting_key: String,
    captions: Option<Captions>,
) {
    let mut joiner = Joiner::new();
    let mut policy = ChurnPolicy::default();
    let mut degraded = Vec::new();
    let mut transcribed = 0usize;

    // **A leg that starts and then delivers nothing.** Every failure this
    // module handles is a leg that *says* it failed; a leg that reports
    // success and then produces no frames was not a case the design had, and
    // it is the worst one available — capture looks healthy, and the Meeting
    // arrives with no audio and nothing to say why. It happens: a CoreAudio
    // process tap can deadlock the microphone's AudioUnit
    // (`.scratch/capture-deadlock`), and both legs then sit silent forever.
    //
    // Silence is not the test — a quiet room still delivers frames of zeros.
    // Zero frames is the test.
    //
    // The leg is also ended, but that is hygiene rather than rescue: the
    // joiner already emits once a leg runs `MAX_LEAD_MS` (400ms) ahead of the
    // other, so a dead leg costs a fraction of a second of lead and not the
    // recording — measured, by deleting the `finish_leg` below and watching
    // the test still pass. Ending it says the leg is *over* rather than late,
    // so the joiner stops holding a margin for something that will never
    // speak again.
    let mut delivered = [false; 2];
    let mut abandoned = [false; 2];
    // How many ticks have passed with the far end audibly playing. The system
    // leg is a process tap: it delivers *nothing at all* when no process is
    // playing — not silence, nothing — so its silence is only evidence of
    // failure once something has been playing through it. The microphone has
    // no such excuse; a quiet room still produces frames of zeros.
    let mut ticks_with_playback = 0u32;
    let mut watchdog = tokio::time::interval(SILENT_LEG_GRACE);
    watchdog.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // The first tick is immediate; the grace period starts after it.
    watchdog.tick().await;

    // Transcription gets its own thread. A whisper window decodes in
    // seconds while the capture channel holds about one second of frames,
    // so decoding inline stalls this drain long enough for CoreAudio to
    // start dropping frames — losing audio that was captured perfectly
    // well. ADR-0019 puts the recording first, and that is only true if
    // the recording never waits for the transcript.
    let transcription = captions.map(|captions| {
        Transcription::spawn(
            TranscriptionPipeline::new(captions.transcriber).in_script(captions.script),
            captions.segments,
        )
    });

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
                _ = watchdog.tick() => {
                    if super::system::output_is_active() == Some(true) {
                        ticks_with_playback += 1;
                    }
                    for (index, channel) in [AudioChannel::Mic, AudioChannel::System]
                        .into_iter()
                        .enumerate()
                    {
                        if delivered[index] || abandoned[index] {
                            continue;
                        }
                        if !judge_silent_leg(channel, ticks_with_playback) {
                            continue;
                        }
                        abandoned[index] = true;
                        let name = channel_name(channel);
                        warn!(
                            ?channel,
                            "this leg started and has delivered nothing; abandoning it so the \
                             other one can still record"
                        );
                        joiner.finish_leg(channel);
                        degraded.push(format!(
                            "{name}: started but delivered no audio at all — the capture \
                             device accepted the request and then produced nothing"
                        ));
                    }
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
            Action::NoteLeg { channel, reason } => {
                info!(
                    ?channel,
                    reason, "capture leg is degraded; it stays attached"
                );
                degraded.push(format!("{}: {reason}", channel_name(channel)));
            }
        }

        if let CaptureEvent::Frame(frame) = &event {
            delivered[leg_index(frame.channel)] = true;
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
    // Read before `finalize`, which consumes the sink. An encoder that never
    // started is the one audio failure that reaches here with nothing else to
    // show for it: no samples were written, so the byte-count check below
    // cannot see it either.
    let encoder_failure = sink.disabled_reason().map(str::to_string);
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
    if let Some(reason) = encoder_failure {
        degraded.push(reason);
    }
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
        segments_tx: mpsc::Sender<TranscribedSegment>,
    ) -> Self {
        // Roughly a minute of blocks. Deep enough to ride out a slow
        // decode, bounded so a hopelessly slow machine loses captions
        // rather than memory.
        let (blocks, incoming) = std::sync::mpsc::sync_channel::<StereoBlock>(4096);
        let dropped = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let handle = std::thread::spawn(move || {
            let mut count = 0usize;
            let send = |segments: Vec<TranscribedSegment>, count: &mut usize| {
                for segment in segments {
                    *count += 1;
                    if segments_tx.blocking_send(segment).is_err() {
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

/// How long a leg may claim to be running before producing a frame.
///
/// Generous: a stream can take a moment to start, and a Meeting that noted a
/// working leg as dead would be worse than one that waited. Frames arrive
/// every 20ms once anything is flowing, so five seconds is two orders of
/// magnitude of headroom.
const SILENT_LEG_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// Whether a leg that has delivered nothing has been given a fair chance.
///
/// The two legs deserve different answers, which is the whole of this
/// function. A microphone always delivers — a silent room produces frames of
/// zeros — so one grace period with nothing at all is already a dead leg. A
/// CoreAudio process tap delivers no callbacks whatsoever until some process
/// plays, so its silence means nothing until the far end has been heard, and
/// abandoning it early would end the system leg of every Meeting that opens
/// with a few quiet seconds. It waits for two grace periods of confirmed
/// playback, so a single tick that catches the start of a sound does not
/// convict it.
///
/// This is the same distinction DECISIONS Q9 drew for the refusal check, and
/// for the same reason: silence with nothing playing is an ordinary quiet
/// meeting.
fn judge_silent_leg(channel: AudioChannel, ticks_with_playback: u32) -> bool {
    match channel {
        AudioChannel::Mic => true,
        AudioChannel::System => ticks_with_playback >= 2,
    }
}

/// Index into the per-leg tracking arrays. Two legs, so an array beats a map.
fn leg_index(channel: AudioChannel) -> usize {
    match channel {
        AudioChannel::Mic => 0,
        AudioChannel::System => 1,
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

    /// An output that refuses every write, so a recording can be driven into
    /// the degraded path. Since ADR-0032's reversal the encoder is in-process
    /// and cannot be missing, so a failing *sink* is the failure that remains.
    struct RefusingWriter;

    impl std::io::Write for RefusingWriter {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("the disk said no"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::other("the disk said no"))
        }
    }

    #[tokio::test]
    async fn a_degraded_leg_keeps_recording_after_it_is_noted() {
        // Q9's whole point. The refusal note is a diagnosis the Core infers,
        // and it has been wrong: a meeting that merely opened quietly was
        // told its system audio was missing. Ending the leg on that made a
        // wrong sentence cost every remaining minute of the far end, so the
        // note must not stop the audio behind it.
        let dir = tempfile::tempdir().expect("tempdir");
        let (source, delivered) = FixtureSource::with_completion(vec![
            Step::audio(AudioChannel::System, 200, 0.0),
            Step::degraded(AudioChannel::System, "arrives as silence"),
            // Everything below here would be lost if the note ended the leg.
            Step::audio(AudioChannel::System, 400, -0.4),
            Step::audio(AudioChannel::Mic, 600, 0.4),
        ]);
        let recorder = Recorder::start_without_transcription(
            Box::new(source),
            dir.path().to_path_buf(),
            "degraded1".to_string(),
        )
        .expect("start");

        delivered.await.expect("the script should finish");
        let outcome = recorder.finish().await;

        assert!(
            outcome
                .degraded
                .iter()
                .any(|note| note.contains("arrives as silence")),
            "the reason must reach the record, got {:?}",
            outcome.degraded
        );
        assert!(
            outcome.seconds >= 0.5,
            "audio after the note must still be recorded, got {:.2}s",
            outcome.seconds
        );
        assert!(
            outcome.audio_path.is_some_and(|path| path.exists()),
            "a degraded leg still produces a file"
        );
    }

    #[tokio::test]
    async fn audio_that_cannot_be_written_reaches_the_record() {
        // The failure this test exists for is silent by construction. A sink
        // that cannot write keeps no samples, so `seconds` is 0.0 and the
        // "captured but no file" check cannot fire either — the Meeting ends
        // up with no audio, no note, and a duration that says a recording
        // happened. The only place the reason exists is the sink, so this
        // proves it gets from there into the outcome.
        //
        // It was a missing ffmpeg binary that used to produce this; since
        // ADR-0032's reversal the encoder is in-process and cannot go
        // missing, so a refusing output is the shape the failure takes now.
        let dir = tempfile::tempdir().expect("tempdir");
        let clock = CaptureClock::start();
        let (events_tx, events_rx) = mpsc::channel::<CaptureEvent>(256);
        let (mut source, delivered) = FixtureSource::with_completion(vec![
            Step::audio(AudioChannel::Mic, 200, 0.4),
            Step::audio(AudioChannel::System, 200, -0.4),
        ]);
        source.start(clock, events_tx).expect("start");

        let sink =
            AudioSink::with_writer(dir.path().join("nocodec1.mp3"), Box::new(RefusingWriter));
        let stop = CancellationToken::new();
        let (finished_tx, finished_rx) = oneshot::channel();
        tokio::spawn(run(
            Box::new(source),
            events_rx,
            sink,
            stop.clone(),
            finished_tx,
            "nocodec1".to_string(),
            None,
        ));

        delivered.await.expect("the script should finish");
        stop.cancel();
        let outcome = finished_rx.await.expect("outcome");

        assert!(outcome.audio_path.is_none(), "nothing written, no file");
        assert!(
            outcome
                .degraded
                .iter()
                .any(|note| note.contains("the disk said no")),
            "the Meeting must say why it has no audio, got {:?}",
            outcome.degraded
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_leg_that_starts_and_delivers_nothing_is_abandoned_not_waited_on() {
        // The capture deadlock's shape (.scratch/capture-deadlock): a leg
        // reports success and then produces no frames, forever. Nothing in the
        // supervisor sees it, because the supervisor reacts to events and
        // there are none.
        //
        // What must happen is that it reaches the record: a Meeting with no
        // audio and no note is indistinguishable from a quiet room, and that
        // is the whole failure.
        //
        // The microphone is the leg under test because it is the one that can
        // be judged on its own: it always delivers, so nothing at all is
        // already proof. The system leg's rule depends on whether anything
        // was playing, which is a property of the machine running the test —
        // `judge_silent_leg` carries that logic and is tested directly.
        //
        // The working leg keeps recording throughout, which is asserted below
        // — though not because of the watchdog. The joiner already emits once
        // a leg leads by `MAX_LEAD_MS`, so a dead leg costs 400ms rather than
        // the meeting. Checked by removing the watchdog's `finish_leg` and
        // watching this still pass; the note is the part with teeth.
        let dir = tempfile::tempdir().expect("tempdir");
        let clock = CaptureClock::start();
        let (events_tx, events_rx) = mpsc::channel::<CaptureEvent>(256);
        // Held for the test's lifetime. A live source keeps its senders open
        // for as long as its streams exist, so the recorder waits; a fixture
        // drops its sender when the script ends, which would end the loop
        // before the watchdog could ever tick.
        let still_capturing = events_tx.clone();

        let (mut source, delivered) = FixtureSource::with_completion(vec![
            Step::audio(AudioChannel::System, 400, 0.4),
            Step::audio(AudioChannel::System, 400, -0.4),
        ]);
        source.start(clock, events_tx).expect("start");

        let sink = AudioSink::new(dir.path(), "silentleg").expect("sink");
        let stop = CancellationToken::new();
        let (finished_tx, finished_rx) = oneshot::channel();
        tokio::spawn(run(
            Box::new(source),
            events_rx,
            sink,
            stop.clone(),
            finished_tx,
            "silentleg".to_string(),
            None,
        ));

        delivered.await.expect("the script should finish");
        // Paused time: this returns at once and moves the clock past the grace
        // period. The yields give the recorder a turn to notice.
        tokio::time::sleep(SILENT_LEG_GRACE * 2).await;
        for _ in 0..32 {
            tokio::task::yield_now().await;
        }

        stop.cancel();
        drop(still_capturing);
        let outcome = finished_rx.await.expect("outcome");

        assert!(
            outcome
                .degraded
                .iter()
                .any(|note| note.contains("microphone") && note.contains("delivered no audio")),
            "the Meeting must say the leg produced nothing, got {:?}",
            outcome.degraded
        );
        assert!(
            outcome.seconds >= 0.7,
            "and the working leg must still have been recorded, got {:.2}s",
            outcome.seconds
        );
        assert!(
            outcome.audio_path.is_some_and(|path| path.exists()),
            "which means a file, not just a duration"
        );
    }

    #[test]
    fn a_silent_system_leg_is_not_convicted_until_something_has_played() {
        // The tap delivers nothing at all while nothing plays, so an early
        // verdict would end the system leg of every Meeting that opens with a
        // few quiet seconds — which my own fix for the capture deadlock made
        // the common case, since the microphone now starts first and the tap
        // sits idle behind it.
        assert!(!judge_silent_leg(AudioChannel::System, 0));
        assert!(
            !judge_silent_leg(AudioChannel::System, 1),
            "one tick may only have caught the start of a sound"
        );
        assert!(judge_silent_leg(AudioChannel::System, 2));

        // The microphone has no such excuse: a silent room still produces
        // frames of zeros, so nothing at all is already a dead leg.
        assert!(judge_silent_leg(AudioChannel::Mic, 0));
    }

    #[tokio::test]
    async fn a_recording_produces_an_audio_file() {
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
