//! What must still be true when transcription misbehaves.
//!
//! Two properties, both of which cost a Meeting if they fail: the last thing
//! said survives pressing stop, and a broken transcriber never takes the
//! recording down with it.

#![cfg(unix)]

use std::sync::Arc;

use anyhow::Result;
use evertranscript_core::asr::Transcriber;
use evertranscript_core::asr::Transcript;
use evertranscript_core::audio::fixture::FixtureSource;
use evertranscript_core::audio::fixture::Step;
use evertranscript_core::Core;
use evertranscript_protocol::AudioChannel;

/// Speech long enough to close chunks, ending mid-utterance with no trailing
/// pause — the shape of a Meeting the Operator stops while someone is still
/// talking.
fn script_ending_mid_sentence() -> Vec<Step> {
    vec![
        Step::audio(AudioChannel::Mic, 4_000, 0.3),
        Step::audio(AudioChannel::Mic, 1_000, 0.0),
        // The tail: speech with no closing silence behind it.
        Step::audio(AudioChannel::Mic, 2_000, 0.3),
    ]
}

/// Counts how many times it was asked to transcribe.
struct CountingTranscriber {
    calls: Arc<std::sync::atomic::AtomicUsize>,
    text: &'static str,
}

impl Transcriber for CountingTranscriber {
    fn transcribe(&mut self, _samples: &[f32], _previous: Option<&str>) -> Result<Transcript> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Transcript {
            text: self.text.to_string(),
            confidence: 0.9,
            decode_time: std::time::Duration::from_millis(1),
        })
    }

    fn describe(&self) -> String {
        "counting".to_string()
    }
}

/// Fails every call, as a model that cannot run would.
struct BrokenTranscriber;

impl Transcriber for BrokenTranscriber {
    fn transcribe(&mut self, _samples: &[f32], _previous: Option<&str>) -> Result<Transcript> {
        anyhow::bail!("the transcription engine died")
    }

    fn describe(&self) -> String {
        "broken".to_string()
    }
}

async fn core_with(
    transcriber: impl Fn() -> Option<Box<dyn Transcriber>> + Send + Sync + 'static,
) -> (Arc<Core>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");
    core.set_source_factory(Arc::new(|| {
        Box::new(FixtureSource::new(script_ending_mid_sentence()))
    }))
    .await;
    core.set_transcriber_factory(Arc::new(transcriber)).await;
    (core, dir)
}

#[tokio::test]
async fn the_last_thing_said_survives_pressing_stop() {
    // Story 5. Without a flush on stop, every Meeting silently loses its
    // final utterance — the one most likely to contain the decision.
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let (core, _dir) = core_with(move || {
        Some(Box::new(CountingTranscriber {
            calls: Arc::clone(&counter),
            text: "the last thing said",
        }))
    })
    .await;

    let meeting = core.start_meeting(None, None).await.expect("start");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    core.stop_meeting().await.expect("stop");
    // The write task is separate from the recorder; give it a moment.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let (_, segments) = core
        .get_meeting(&meeting.id)
        .await
        .expect("get")
        .expect("the Meeting");

    let transcribed = calls.load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        transcribed >= 2,
        "the trailing utterance must be flushed and transcribed, not dropped \
         (only {transcribed} chunks reached the engine)"
    );
    assert!(
        segments.len() >= 2,
        "the flushed tail must reach the record, got {} segments",
        segments.len()
    );
}

#[tokio::test]
async fn a_dead_transcriber_does_not_stop_the_recording() {
    // ADR-0029 as amended: capture never depends on ASR health. Losing
    // captions is a degraded meeting; losing the recording is a lost one.
    let (core, dir) = core_with(|| Some(Box::new(BrokenTranscriber))).await;

    let meeting = core
        .start_meeting(None, Some("Zoom".into()))
        .await
        .expect("start");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let stopped = core
        .stop_meeting()
        .await
        .expect("the Meeting must still stop cleanly");

    assert_eq!(stopped.id, meeting.id);
    assert!(
        stopped.ended_at.is_some(),
        "the Meeting must persist even with no transcript"
    );

    let (_, segments) = core
        .get_meeting(&meeting.id)
        .await
        .expect("get")
        .expect("the Meeting");
    assert!(
        segments.is_empty(),
        "a failing engine produces no transcript — and says nothing false"
    );

    // The Mirror still exists, which is what makes this a recorded Meeting
    // rather than a lost one.
    let mirrors: Vec<String> = std::fs::read_dir(dir.path().join("History"))
        .expect("read history")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.ends_with(".md"))
        .collect();
    assert_eq!(mirrors.len(), 1, "the Meeting must still have its Mirror");
}

#[tokio::test]
async fn a_meeting_with_no_transcriber_at_all_still_records() {
    // The state of a fresh install before the model has downloaded.
    let (core, _dir) = core_with(|| None).await;

    let meeting = core.start_meeting(None, None).await.expect("start");
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let stopped = core.stop_meeting().await.expect("stop");

    assert_eq!(stopped.id, meeting.id);
    assert!(
        stopped.ended_at.is_some(),
        "never missing a meeting outranks transcribing it (ADR-0019, ADR-0023)"
    );
}
