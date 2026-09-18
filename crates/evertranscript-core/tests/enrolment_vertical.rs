//! The Operator's enrolment, end to end, over the real models.
//!
//! The rest of the enrolment's tests are fixture-driven and prove the parts:
//! `diarize::enrol` judges a scripted diarization, `audio::enrol` records
//! from a scripted source, `store::speakers` writes and reads the row. What
//! none of them touch is the glue — decoding the kept mp3, loading the real
//! ONNX pair, and the claim `Core::remint_the_enrolment` exists to make: that
//! a voice-model change costs the Operator nothing, because the recording
//! survives the wipe that takes the vectors (Q292, answering Q236).
//!
//! Two things that *do* need a person are still not here, and are the whole
//! of what is left: a real microphone with a TCC grant, and a real human
//! voice. Everything below that — the capture seam downwards — runs here,
//! because `Core::source_factory` reaches the enrolment since Q295 and
//! `evertranscript_fixtures::OPERATOR_ENROLMENT` is one voice for half a
//! minute, which is the shape `diarize::enrol` accepts and nothing else in
//! the repo had.
//!
//! **Skipped loudly when the models are absent.** A machine with no ONNX
//! pair must not report this as passing: the whole point is that the real
//! model ran.

use std::sync::Arc;

use evertranscript_core::Core;
use evertranscript_core::audio::fixture::FixtureSource;
use evertranscript_core::audio::fixture::Step;
use evertranscript_protocol::AudioChannel;

/// A directory holding the pair under the names `Core` looks for.
///
/// `EVERTRANSCRIPT_DIARIZE_MODELS` first, so a machine that keeps them
/// elsewhere can say so; then the real install, which is where they are on a
/// developer's Mac. Note the filenames differ from the ones
/// `diarize::live`'s own gate wants — that gate loads `LiveDiarizer`
/// directly, this one goes through `Core`, which names them `diarize-*`.
fn models_dir() -> Option<std::path::PathBuf> {
    let candidates = [
        std::env::var_os("EVERTRANSCRIPT_DIARIZE_MODELS").map(std::path::PathBuf::from),
        Some(evertranscript_core::paths::models_dir()),
    ];
    candidates.into_iter().flatten().find(|dir| {
        dir.join("diarize-segmentation.onnx").exists()
            && dir.join("diarize-embedding.onnx").exists()
    })
}

macro_rules! models_or_skip {
    () => {
        match models_dir() {
            Some(dir) => dir,
            None => {
                eprintln!(
                    "SKIPPED: no diarize-segmentation.onnx / diarize-embedding.onnx in \
                     EVERTRANSCRIPT_DIARIZE_MODELS or {}",
                    evertranscript_core::paths::models_dir().display()
                );
                return;
            }
        }
    };
}

/// A Core whose Briefing is acknowledged and whose models are the real ones.
fn core_with_models(history_dir: std::path::PathBuf, models: std::path::PathBuf) -> Arc<Core> {
    let settings_path = history_dir.join(".settings-test.json");
    std::fs::create_dir_all(&history_dir).expect("history dir");
    std::fs::write(
        &settings_path,
        serde_json::to_string(&serde_json::json!({ "briefingAcknowledged": true }))
            .expect("settings"),
    )
    .expect("write settings");
    Core::with_paths_and_models(history_dir, settings_path, models).expect("core")
}

/// Opens the machine store the way a migration would, for the one thing no
/// public method does: taking the vectors as a model change takes them.
fn store(history_dir: &std::path::Path) -> rusqlite::Connection {
    rusqlite::Connection::open(
        history_dir
            .join(evertranscript_core::paths::DATA_DIR_NAME)
            .join("EverTranscript.db"),
    )
    .expect("open the machine store")
}

#[tokio::test(flavor = "multi_thread")]
async fn an_enrolment_survives_the_model_change_that_takes_every_vector() {
    let models = models_or_skip!();
    let dir = tempfile::tempdir().expect("tempdir");
    let history_dir = dir.path().join("History");
    let core = core_with_models(history_dir.clone(), models);

    // The clip plays on the microphone leg, at the rate capture runs at. A
    // `System` step goes in beside it: the enrolment must ignore the far end
    // even when there is one, and on the live path `start_microphone_only`
    // is what guarantees that — here the assertion is `voices_heard == 1`
    // despite a second voice being offered.
    let mic = evertranscript_fixtures::OPERATOR_ENROLMENT.samples_at(48_000);
    let far_end = evertranscript_fixtures::ENGLISH_MEETING.samples_at(48_000);
    let clip_ms = (mic.duration_seconds() * 1000.0) as u64;
    core.set_source_factory(Arc::new(move || {
        Box::new(FixtureSource::new(vec![
            Step::Samples {
                channel: AudioChannel::Mic,
                samples: mic.data.clone(),
            },
            Step::Samples {
                channel: AudioChannel::System,
                samples: far_end.data.clone(),
            },
        ]))
    }))
    .await;

    // Two seconds of wall clock, half a minute of audio: the fixture source
    // delivers its whole script as fast as the channel takes it, which is
    // the documented reason it exists. The recording's length comes from the
    // samples, never from the clock.
    let enrolled = core.enrol_operator(2).await.expect("enrol");
    assert!(
        enrolled.accepted,
        "the real model refused a clean one-voice clip: {:?} {:?}",
        enrolled.refusal, enrolled.detail
    );
    assert_eq!(
        enrolled.voices_heard, 1,
        "one voice, and the far end ignored"
    );
    assert!(
        enrolled.voiced_ms >= evertranscript_core::diarize::operator::MIN_OPERATOR_MS,
        "voiced {} ms of {clip_ms} ms wall clock, floor is {}",
        enrolled.voiced_ms,
        evertranscript_core::diarize::operator::MIN_OPERATOR_MS
    );
    eprintln!(
        "MEASURED enrolment: voiced_ms={} of {clip_ms} ms wall clock ({:.0}%), voices={}",
        enrolled.voiced_ms,
        100.0 * enrolled.voiced_ms as f64 / clip_ms as f64,
        enrolled.voices_heard
    );

    let before = core
        .operator_enrolment()
        .await
        .expect("enrolment")
        .enrolment
        .expect("a row after enrolling");
    assert!(before.active, "enrolled, in this model's space");
    let kept = history_dir.join(
        store(&history_dir)
            .query_row("SELECT audio_path FROM speaker_enrolments", [], |row| {
                row.get::<_, String>(0)
            })
            .expect("the enrolment's audio path"),
    );
    assert!(kept.exists(), "the kept recording is on disk at {kept:?}");

    // ---- the model change ----
    //
    // Exactly what migration 15 does: every vector and every exemplar. The
    // recording is not a vector, so it stays, and that is the whole of the
    // claim under test.
    store(&history_dir)
        .execute_batch(evertranscript_core::store::schema::MODEL_CHANGE_WIPE)
        .expect("wipe");

    let wiped = core
        .operator_enrolment()
        .await
        .expect("enrolment")
        .enrolment
        .expect("the row outlives the vectors");
    assert!(
        !wiped.active,
        "with the vectors gone the enrolment is not yet the identity again"
    );
    assert!(kept.exists(), "the wipe does not take the recording");

    let started = std::time::Instant::now();
    core.rerun_if_the_model_changed().await;
    let remint = started.elapsed();

    let after = core
        .operator_enrolment()
        .await
        .expect("enrolment")
        .enrolment
        .expect("a row after the re-mint");
    assert!(
        after.active,
        "the Operator is recognized again before any Meeting is walked"
    );
    assert_eq!(
        (after.duration_ms, &after.recorded_at, &after.speaker_id),
        (before.duration_ms, &before.recorded_at, &before.speaker_id),
        "the re-mint replaces the vectors, not the record of the act"
    );
    let voiceprint: Option<Vec<u8>> = store(&history_dir)
        .query_row(
            "SELECT voiceprint FROM speakers WHERE id = ?1",
            [&after.speaker_id],
            |row| row.get(0),
        )
        .expect("the Operator's row");
    assert!(
        voiceprint.is_some_and(|vector| !vector.is_empty()),
        "a Voiceprint in the new space"
    );
    eprintln!("MEASURED re-mint: {} ms", remint.as_millis());

    // ---- and it plays back ----
    //
    // The below-socket half of the plan's end-to-end item: the Registry row
    // holding the one biometric the Operator deliberately gave is also the
    // one that used to dead-end, because `sample_source` needs a Meeting and
    // an enrolment has none (Q293).
    let sample = core
        .speaker_sample(&after.speaker_id)
        .await
        .expect("sample")
        .sample
        .expect("the enrolment plays back");
    assert!(!sample.audio_base64.is_empty());
    assert_eq!(sample.mime_type, "audio/mpeg");
    assert!(
        sample.meeting_id.is_none(),
        "cut from the enrolment, which is not a Meeting"
    );
}
