//! Real speech through the whole capture vertical, verified by ear-shaped
//! assertions rather than byte equality.
//!
//! This is the harness the PRD's testing philosophy describes: fixture audio
//! enters at the AudioSource seam, travels the real joiner and the real
//! ffmpeg sink, and the file that lands on disk is decoded back and compared
//! to what went in. Later milestones extend the same path through ASR
//! (ticket 06) and diarization.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use evertranscript_core::Core;
use evertranscript_core::audio::SAMPLE_RATE;
use evertranscript_core::audio::fixture::FixtureSource;
use evertranscript_core::audio::fixture::Step;
use evertranscript_core::audio::sink::ffmpeg_available;
use evertranscript_fixtures::BILINGUAL_MEETING;
use evertranscript_fixtures::ENGLISH_MEETING;
use evertranscript_fixtures::Fixture;
use evertranscript_fixtures::similarity::Features;
use evertranscript_protocol::AudioChannel;

async fn skip_without_ffmpeg() -> bool {
    if ffmpeg_available().await {
        return false;
    }
    eprintln!("skipping: ffmpeg is not available on this machine");
    true
}

/// Decodes an encoded file back to mono f32 so it can be compared with what
/// was recorded. `channel` picks one side of the stereo pair.
fn decode(path: &Path, channel: AudioChannel) -> Vec<f32> {
    let wav = path.with_extension("decoded.wav");
    let map = match channel {
        // Left is the microphone, right is system audio (ADR-0032).
        AudioChannel::Mic => "pan=mono|c0=c0",
        AudioChannel::System => "pan=mono|c0=c1",
    };
    let status = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(path)
        .args(["-af", map, "-ar", "16000", "-c:a", "pcm_s16le", "-y"])
        .arg(&wav)
        .status()
        .expect("running ffmpeg to decode");
    assert!(status.success(), "decoding {} failed", path.display());

    let reader = hound::WavReader::open(&wav).expect("open decoded wav");
    let scale = 1.0 / 32768.0;
    reader
        .into_samples::<i16>()
        .map(|sample| sample.expect("sample") as f32 * scale)
        .collect()
}

/// Records one fixture on one channel and returns the finished audio file.
async fn record_fixture(fixture: Fixture, channel: AudioChannel) -> (PathBuf, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let history_dir = dir.path().join("History");
    let core = Core::with_history_dir_acknowledged(history_dir.clone()).expect("core");

    // Capture runs at 48 kHz; the fixtures are stored at 16 kHz.
    let samples = fixture.samples_at(SAMPLE_RATE).data;
    core.set_source_factory(Arc::new(move || {
        Box::new(FixtureSource::new(vec![Step::Samples {
            channel,
            samples: samples.clone(),
        }]))
    }))
    .await;

    let meeting = core
        .start_meeting(None, Some("Zoom".to_string()))
        .await
        .expect("start");
    // Let the scripted source deliver.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let stopped = core.stop_meeting().await.expect("stop");
    assert_eq!(stopped.id, meeting.id);

    let relative = stopped
        .audio_path
        .expect("the recording must produce an audio file");
    (history_dir.join(relative), dir)
}

#[tokio::test]
async fn english_speech_survives_the_capture_pipeline() {
    if skip_without_ffmpeg().await {
        return;
    }
    let (audio_path, _dir) = record_fixture(ENGLISH_MEETING, AudioChannel::Mic).await;

    let original = ENGLISH_MEETING.samples();
    let recorded = decode(&audio_path, AudioChannel::Mic);

    let before = Features::of(&original.data, original.rate);
    let after = Features::of(&recorded, 16_000);

    // AAC at 192 kbps through a resample is not bit-identical and never will
    // be; what must hold is that it still sounds like the same speech.
    before.assert_similar(&after, 0.20, "English speech through capture and AAC");

    // And the speech band is where the energy is, which is what makes this
    // audio rather than noise that happens to have the right level.
    assert!(
        after.band_energy[1] > 0.35,
        "recorded speech should keep its energy in the speech band, got {:?}",
        after.band_energy
    );
}

#[tokio::test]
async fn mandarin_and_english_survive_the_capture_pipeline() {
    if skip_without_ffmpeg().await {
        return;
    }
    // Code-switching is the Operator's normal case (story 7), so it gets the
    // same end-to-end proof as English rather than being assumed.
    let (audio_path, _dir) = record_fixture(BILINGUAL_MEETING, AudioChannel::Mic).await;

    let original = BILINGUAL_MEETING.samples();
    let recorded = decode(&audio_path, AudioChannel::Mic);

    Features::of(&original.data, original.rate).assert_similar(
        &Features::of(&recorded, 16_000),
        0.20,
        "Mandarin/English speech through capture and AAC",
    );
}

#[tokio::test]
async fn the_channels_stay_on_their_own_sides() {
    if skip_without_ffmpeg().await {
        return;
    }
    // Left is the mic, right is system audio. Getting this backwards would
    // silently invert every attribution the moment diarization lands, so it
    // is asserted rather than assumed.
    let (audio_path, _dir) = record_fixture(ENGLISH_MEETING, AudioChannel::Mic).await;

    let mic_side = Features::of(&decode(&audio_path, AudioChannel::Mic), 16_000);
    let system_side = Features::of(&decode(&audio_path, AudioChannel::System), 16_000);

    assert!(
        mic_side.rms > 0.01,
        "the microphone side should carry the speech, got rms {}",
        mic_side.rms
    );
    assert!(
        system_side.rms < mic_side.rms / 4.0,
        "nothing was captured on the system leg, so its side should be near-silent \
         (mic {:.4} vs system {:.4})",
        mic_side.rms,
        system_side.rms
    );
}

#[tokio::test]
async fn a_recording_lasts_as_long_as_what_went_into_it() {
    if skip_without_ffmpeg().await {
        return;
    }
    // The property the joiner exists to guarantee: audio length tracks the
    // capture timeline, so a timestamp means the same thing in the audio as
    // in the transcript.
    let (audio_path, _dir) = record_fixture(ENGLISH_MEETING, AudioChannel::Mic).await;

    let expected = ENGLISH_MEETING.samples().duration_seconds();
    let recorded = decode(&audio_path, AudioChannel::Mic).len() as f64 / 16_000.0;

    assert!(
        (recorded - expected).abs() < 0.5,
        "recorded {recorded:.2}s of a {expected:.2}s fixture — the timeline drifted"
    );
}
