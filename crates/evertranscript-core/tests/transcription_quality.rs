//! Real transcription of real speech, scored.
//!
//! The PRD names whisper.cpp quality on the Operator's actual languages as
//! the top unverified risk. This is where that risk starts being measured
//! rather than asserted: fixture audio goes through the real engine and the
//! WER/CER come out as numbers the test prints on every run.
//!
//! Needs a model. Set `EVERTRANSCRIPT_TEST_MODEL` to a ggml whisper model;
//! without it these skip rather than fail, so CI stays green on machines
//! that have not downloaded one.

#![cfg(unix)]

use std::path::PathBuf;

use evertranscript_core::asr::whisper::Language;
use evertranscript_core::asr::whisper::WhisperEngine;
use evertranscript_core::asr::whisper::WHISPER_RATE;
use evertranscript_core::asr::Transcriber;
use evertranscript_fixtures::wer::character_error_rate;
use evertranscript_fixtures::wer::word_error_rate;
use evertranscript_fixtures::Fixture;
use evertranscript_fixtures::BILINGUAL_MEETING;
use evertranscript_fixtures::ENGLISH_MEETING;
use evertranscript_fixtures::ROOM_NOISE;
use evertranscript_fixtures::SILENCE;

fn test_model() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var("EVERTRANSCRIPT_TEST_MODEL").ok()?);
    path.exists().then_some(path)
}

fn engine(language: Language) -> Option<WhisperEngine> {
    let path = test_model()?;
    Some(WhisperEngine::load_with(&path, language).expect("the model should load"))
}

fn transcribe(fixture: Fixture, language: Language) -> Option<String> {
    let mut engine = engine(language)?;
    let samples = fixture.samples_at(WHISPER_RATE);
    let result = engine
        .transcribe(&samples.data, None)
        .expect("transcription should not error");
    Some(result.text)
}

#[test]
fn english_speech_transcribes_with_a_reported_error_rate() {
    let Some(text) = transcribe(ENGLISH_MEETING, Language::Fixed("en".into())) else {
        eprintln!("skipping: set EVERTRANSCRIPT_TEST_MODEL to run transcription tests");
        return;
    };

    let rate = word_error_rate(ENGLISH_MEETING.transcript, &text);
    println!("\n  English WER: {rate}\n  heard: {text}\n");

    assert!(!text.trim().is_empty(), "clear speech must produce text");
    // Deliberately loose: this gate exists to catch a pipeline that has
    // stopped transcribing at all, not to certify quality. The printed
    // number is the deliverable; tightening this into a quality bar needs
    // real recorded meetings, not synthesized ones.
    assert!(
        rate.rate() < 0.8,
        "transcription looks broken rather than merely imperfect: {rate}"
    );
}

#[test]
fn code_switching_speech_transcribes_with_a_reported_error_rate() {
    // Story 7: Mandarin and English in one meeting is this Operator's normal
    // case, and it is what drove the large-v3-turbo model choice. A small
    // model will score badly here; the point is that the number is visible.
    let Some(text) = transcribe(BILINGUAL_MEETING, Language::Auto) else {
        eprintln!("skipping: set EVERTRANSCRIPT_TEST_MODEL to run transcription tests");
        return;
    };

    let characters = character_error_rate(BILINGUAL_MEETING.transcript, &text);
    let words = word_error_rate(BILINGUAL_MEETING.transcript, &text);
    println!("\n  Bilingual CER: {characters}\n  Bilingual WER: {words}\n  heard: {text}\n");

    assert!(
        !text.trim().is_empty(),
        "code-switched speech must produce something"
    );
}

#[test]
fn silence_does_not_become_a_permanent_line_in_the_record() {
    // The canary that matters most. Whisper's signature failure is inventing
    // "Thank you for watching" from nothing, and our record is immutable —
    // an invention that lands here is in the Operator's History forever.
    for fixture in [SILENCE, ROOM_NOISE] {
        let Some(text) = transcribe(fixture, Language::Fixed("en".into())) else {
            eprintln!("skipping: set EVERTRANSCRIPT_TEST_MODEL to run transcription tests");
            return;
        };
        println!("\n  {} produced: {text:?}\n", fixture.name);

        // What the raw engine emits here is whisper's business; what matters
        // is that the pipeline's filters reject it. That end-to-end
        // assertion lives in the pipeline test below.
        let rate = word_error_rate(fixture.transcript, &text);
        if !text.trim().is_empty() {
            println!(
                "  note: the raw engine hallucinated on {} (WER {rate}); \
                 the pipeline filters must catch this",
                fixture.name
            );
        }
    }
}

#[test]
fn the_pipeline_rejects_what_the_engine_invents_on_silence() {
    use evertranscript_core::asr::pipeline::TranscriptionPipeline;
    use evertranscript_core::audio::joiner::StereoBlock;
    use evertranscript_core::audio::CaptureOffset;
    use evertranscript_core::audio::SAMPLE_RATE;

    let Some(model) = test_model() else {
        eprintln!("skipping: set EVERTRANSCRIPT_TEST_MODEL to run transcription tests");
        return;
    };
    let engine = WhisperEngine::load_with(&model, Language::Fixed("en".into())).expect("load");
    let mut pipeline = TranscriptionPipeline::new(Box::new(engine));

    // Feed real silence through the whole pipeline at capture rate.
    let samples = SILENCE.samples_at(SAMPLE_RATE);
    let mut interleaved = Vec::with_capacity(samples.data.len() * 2);
    for sample in &samples.data {
        interleaved.push(*sample);
        interleaved.push(*sample);
    }
    let mut segments = pipeline.push(&StereoBlock {
        offset: CaptureOffset::ZERO,
        samples: interleaved,
    });
    segments.extend(pipeline.flush());

    assert!(
        segments.is_empty(),
        "silence must reach the record as nothing at all, got {segments:?}"
    );
}

#[test]
fn real_speech_survives_the_whole_pipeline_into_segments() {
    use evertranscript_core::asr::pipeline::TranscriptionPipeline;
    use evertranscript_core::audio::joiner::StereoBlock;
    use evertranscript_core::audio::CaptureOffset;
    use evertranscript_core::audio::SAMPLE_RATE;
    use evertranscript_protocol::AudioChannel;

    let Some(model) = test_model() else {
        eprintln!("skipping: set EVERTRANSCRIPT_TEST_MODEL to run transcription tests");
        return;
    };
    let engine = WhisperEngine::load_with(&model, Language::Fixed("en".into())).expect("load");
    let mut pipeline = TranscriptionPipeline::new(Box::new(engine));

    // Speech on the mic leg only, at capture rate, as a real recording would
    // arrive.
    let samples = ENGLISH_MEETING.samples_at(SAMPLE_RATE);
    let mut interleaved = Vec::with_capacity(samples.data.len() * 2);
    for sample in &samples.data {
        interleaved.push(*sample);
        interleaved.push(0.0);
    }
    let mut segments = pipeline.push(&StereoBlock {
        offset: CaptureOffset::ZERO,
        samples: interleaved,
    });
    segments.extend(pipeline.flush());

    assert!(
        !segments.is_empty(),
        "speech must produce transcript segments"
    );
    assert!(
        segments
            .iter()
            .all(|segment| segment.channel == AudioChannel::Mic),
        "nothing was on the system leg, so nothing may be attributed to it"
    );
    for segment in &segments {
        assert!(
            segment.end_ms > segment.start_ms,
            "segments need real spans"
        );
    }

    let combined = segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let rate = word_error_rate(ENGLISH_MEETING.transcript, &combined);
    println!("\n  end-to-end WER: {rate}\n  heard: {combined}\n");
}
