//! The Operator's script choice has to reach the record.
//!
//! Every link in that chain is tested somewhere already — `filters::clean`
//! honours the script, and `live_captions` proves segments reach the store —
//! but nothing joined them up, so the wiring between settings and the
//! transcript was the one part nobody checked. It is also the part most
//! likely to break silently: a `Captions` assembled with the wrong field, or
//! a default quietly substituted, produces a Meeting that records perfectly
//! and writes it in the wrong script.

#![cfg(unix)]

use std::sync::Arc;

use anyhow::Result;
use evertranscript_core::Core;
use evertranscript_core::asr::Transcriber;
use evertranscript_core::asr::Transcript;
use evertranscript_core::audio::fixture::FixtureSource;
use evertranscript_core::audio::fixture::Step;
use evertranscript_protocol::AudioChannel;
use evertranscript_protocol::ChineseScript;
use evertranscript_protocol::SettingsSetParams;

/// Returns the same Mandarin sentence for every chunk, in one fixed script.
///
/// Fixed on purpose: the engine's own preference is what this setting
/// exists to override, so the test supplies the script the model would have
/// chosen and asks what the record ends up saying.
struct SpeaksMandarin {
    text: &'static str,
}

impl Transcriber for SpeaksMandarin {
    fn transcribe(&mut self, _samples: &[f32], _previous: Option<&str>) -> Result<Transcript> {
        Ok(Transcript {
            text: self.text.to_string(),
            confidence: 0.9,
            decode_time: std::time::Duration::from_millis(1),
        })
    }

    fn describe(&self) -> String {
        "mandarin".to_string()
    }
}

/// Enough speech, with a pause behind it, to close at least one chunk.
fn speech() -> Vec<Step> {
    vec![
        Step::audio(AudioChannel::Mic, 4_000, 0.3),
        Step::audio(AudioChannel::Mic, 1_500, 0.0),
    ]
}

async fn record_saying(text: &'static str, script: Option<ChineseScript>) -> Vec<String> {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");
    core.set_source_factory(Arc::new(|| Box::new(FixtureSource::new(speech()))))
        .await;
    core.set_transcriber_factory(Arc::new(move || {
        Some(Box::new(SpeaksMandarin { text }) as Box<dyn Transcriber>)
    }))
    .await;

    if let Some(script) = script {
        core.update_settings(SettingsSetParams {
            chinese_script: Some(script),
            ..Default::default()
        })
        .await
        .expect("settings");
    }

    let meeting = core.start_meeting(None, None).await.expect("start");
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    core.stop_meeting().await.expect("stop");
    // Segments are persisted by a task of their own.
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    let (_, segments) = core
        .get_meeting(&meeting.id)
        .await
        .expect("get")
        .expect("the Meeting");
    segments.into_iter().map(|segment| segment.text).collect()
}

#[tokio::test]
async fn a_traditional_decode_is_recorded_simplified_by_default() {
    // What shipped, and what the dogfood run measured going wrong: the
    // model returns Traditional for a Simplified speaker.
    let recorded = record_saying("會議決定推遲投票", None).await;
    assert!(!recorded.is_empty(), "the Meeting produced no transcript");
    for line in &recorded {
        assert_eq!(
            line, "会议决定推迟投票",
            "the default record is Simplified, got {line}"
        );
    }
}

#[tokio::test]
async fn an_operator_who_chose_traditional_gets_it_in_the_record() {
    // The half that proves the setting is carried rather than defaulted: a
    // Simplified decode has to come back Traditional, which cannot happen
    // by accident.
    let recorded = record_saying("会议决定推迟投票", Some(ChineseScript::Traditional)).await;
    assert!(!recorded.is_empty(), "the Meeting produced no transcript");
    for line in &recorded {
        assert_eq!(
            line, "會議決定推遲投票",
            "the Operator asked for Traditional, got {line}"
        );
    }
}
