//! Running Diarization over a finished Meeting.
//!
//! This is the piece that makes the rest of the module something the product
//! does rather than something it contains. Without it every part of M3 would
//! be correct, tested, and never called — which is the failure this project
//! has now found six times in two milestones, always in code that compiled.
//!
//! The batch policy is the catalog's, and each rule is here for a failure
//! that has already happened somewhere:
//!
//! - **Reject, do not queue.** A second request while one is running is
//!   refused. M1 shipped a version of the opposite where transcription
//!   starved capture (DECISIONS Q7); post-meeting work is the lowest-value
//!   thing this process does and must never accumulate.
//! - **`catch_unwind` around the models.** ONNX Runtime is C++ behind an FFI
//!   boundary. A panic crossing it takes the Core down, and with it the
//!   recording of whatever meeting started while this was running.
//! - **Cancellable, at every span.** A multi-minute job on someone's laptop
//!   that cannot be stopped is not a thing this product gets to have.

use std::path::Path;
use std::sync::Mutex;

use anyhow::Result;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use super::Cancel;
use super::DiarizeError;
use super::Diarizer;
use super::MeetingAudio;
use super::Progress;
use super::fbank::SAMPLE_RATE;

/// The two channels of a finished Meeting, at the rate the models want.
pub struct DecodedMeeting {
    pub mic: Vec<f32>,
    pub system: Vec<f32>,
}

impl DecodedMeeting {
    pub fn audio(&self) -> MeetingAudio<'_> {
        MeetingAudio {
            mic: &self.mic,
            system: &self.system,
            sample_rate: SAMPLE_RATE,
        }
    }
}

/// Reads a Meeting's stereo AAC back into two mono channels at 16 kHz.
///
/// **Left is mic, right is system** — the convention `audio::sink` writes and
/// the one thing here that would be catastrophic to get backwards: swapping
/// them attributes every word the Operator said to the far end and vice
/// versa, in a record that is immutable by design.
pub fn decode(path: &Path) -> Result<DecodedMeeting> {
    let file = std::fs::File::open(path)?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());

    // The hint follows the file rather than being fixed: Meetings recorded
    // before ADR-0032's 2026-09-05 reversal are `.m4a`, and ones after are
    // `.mp3`. Both keep playing forever (ADR-0009), so both must probe.
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
        hint.with_extension(extension);
    }
    let probed = symphonia::default::get_probe().format(
        &hint,
        stream,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;
    let mut format = probed.format;

    let track = format
        .tracks()
        .iter()
        .find(|track| track.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or_else(|| anyhow::anyhow!("no audio track in {}", path.display()))?;
    let track_id = track.id;
    let source_rate = track.codec_params.sample_rate.unwrap_or(SAMPLE_RATE);

    let mut decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;

    let mut mic = Vec::new();
    let mut system = Vec::new();

    // `next_packet` erroring is end-of-stream, and is also how symphonia
    // reports a truncated file — which a crash mid-recording produces.
    // Whatever decoded is kept rather than the Meeting being unusable.
    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != track_id {
            continue;
        }
        let Ok(decoded) = decoder.decode(&packet) else {
            continue;
        };
        let spec = *decoded.spec();
        let mut buffer =
            symphonia::core::audio::SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buffer.copy_interleaved_ref(decoded);
        let channels = spec.channels.count().max(1);
        for frame in buffer.samples().chunks(channels) {
            mic.push(frame[0]);
            // A mono file is one voice on both legs rather than a missing
            // one: better to diarize it twice than to silently drop half a
            // Meeting.
            system.push(if channels > 1 { frame[1] } else { frame[0] });
        }
    }

    Ok(DecodedMeeting {
        mic: resample_to_model_rate(&mic, source_rate),
        system: resample_to_model_rate(&system, source_rate),
    })
}

/// Linear resampling to the rate the models were trained at.
///
/// Linear rather than a windowed sinc, and that is a deliberate limit: this
/// feeds speaker embeddings, which care about spectral envelope over
/// hundreds of milliseconds, not about the imaging artefacts a cheap
/// resampler leaves above 7 kHz — and the mel bank stops at 7.6 kHz anyway.
/// The transcription path uses `rubato` where it matters.
fn resample_to_model_rate(samples: &[f32], from_rate: u32) -> Vec<f32> {
    if from_rate == SAMPLE_RATE || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = SAMPLE_RATE as f64 / from_rate as f64;
    let count = (samples.len() as f64 * ratio) as usize;
    (0..count)
        .map(|index| {
            let source = index as f64 / ratio;
            let left = source.floor() as usize;
            let right = (left + 1).min(samples.len() - 1);
            let fraction = (source - left as f64) as f32;
            samples[left.min(samples.len() - 1)] * (1.0 - fraction) + samples[right] * fraction
        })
        .collect()
}

/// At most one Diarization at a time, per process.
///
/// A lock rather than a queue, because the policy is to refuse: a Meeting
/// that could not be diarized now can be diarized later on the Operator's
/// say-so, and a backlog of them competing for the machine is strictly
/// worse than none.
static RUNNING: Mutex<Option<String>> = Mutex::new(None);

/// Refused because another Meeting is already being diarized.
#[derive(Debug, thiserror::Error)]
#[error("diarization is already running for {0}")]
pub struct Busy(pub String);

/// Claims the single diarization slot for the duration of a run.
pub struct Slot;

impl Slot {
    pub fn claim(meeting_id: &str) -> Result<Self, Busy> {
        let mut running = RUNNING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match running.as_ref() {
            Some(other) => Err(Busy(other.clone())),
            None => {
                *running = Some(meeting_id.to_string());
                Ok(Self)
            }
        }
    }

    pub fn current() -> Option<String> {
        RUNNING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        // Released on panic as well as on return: a poisoned lock that
        // permanently refused every later run would turn one bad Meeting
        // into a permanently broken feature.
        *RUNNING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

/// Runs a Diarizer with the FFI boundary guarded.
///
/// ONNX Runtime is C++. A panic unwinding out of it would take down a
/// process that may be recording a different meeting at that moment, and
/// losing a live recording to a post-meeting job is the worst trade this
/// product could make.
pub fn run_guarded(
    diarizer: &mut dyn Diarizer,
    audio: MeetingAudio<'_>,
    progress: &mut dyn FnMut(Progress),
    cancel: &Cancel,
) -> Result<super::Diarization, DiarizeError> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        diarizer.diarize(audio, progress, cancel)
    }));
    match result {
        Ok(outcome) => outcome,
        Err(_) => Err(DiarizeError::Unavailable(
            "the diarization models panicked; the Meeting is unattributed and the recording is \
             untouched"
                .to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diarize::fixture::FixtureDiarizer;

    #[test]
    fn a_second_run_is_refused_rather_than_queued() {
        // The catalog's reject-don't-queue. M1 already paid for the opposite
        // shape, where post-meeting work starved live capture.
        let first = Slot::claim("meeting-a").expect("claims");
        let second = Slot::claim("meeting-b");
        assert!(second.is_err());
        assert_eq!(Slot::current().as_deref(), Some("meeting-a"));
        drop(first);
        assert!(Slot::claim("meeting-b").is_ok());
    }

    #[test]
    fn the_slot_is_released_even_if_the_run_panics() {
        // Otherwise one bad Meeting turns Diarization off permanently, and
        // the only symptom is that nothing is ever attributed again.
        let outcome = std::panic::catch_unwind(|| {
            let _slot = Slot::claim("panicking").expect("claims");
            panic!("models exploded");
        });
        assert!(outcome.is_err());
        assert_eq!(Slot::current(), None, "the slot came back");
    }

    #[test]
    fn a_panicking_model_is_unavailable_rather_than_a_dead_process() {
        // The Core may be recording a different meeting at this moment.
        // Losing a live recording to a post-meeting job is the worst trade
        // available.
        struct Exploding;
        impl Diarizer for Exploding {
            fn diarize(
                &mut self,
                _audio: MeetingAudio<'_>,
                _progress: &mut dyn FnMut(Progress),
                _cancel: &Cancel,
            ) -> Result<super::super::Diarization, DiarizeError> {
                panic!("segfault-adjacent");
            }
            fn describe(&self) -> String {
                "exploding".into()
            }
        }

        let silence = vec![0.0_f32; 16_000];
        let audio = MeetingAudio {
            mic: &silence,
            system: &silence,
            sample_rate: SAMPLE_RATE,
        };
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let result = run_guarded(&mut Exploding, audio, &mut |_| {}, &Cancel::new());
        std::panic::set_hook(previous);

        assert!(matches!(result, Err(DiarizeError::Unavailable(_))));
    }

    #[test]
    fn a_healthy_run_passes_its_result_through_the_guard() {
        let silence = vec![0.0_f32; 16_000];
        let audio = MeetingAudio {
            mic: &silence,
            system: &silence,
            sample_rate: SAMPLE_RATE,
        };
        let result = run_guarded(
            &mut FixtureDiarizer::clean_two_speaker(),
            audio,
            &mut |_| {},
            &Cancel::new(),
        )
        .expect("runs");
        assert!(!result.turns.is_empty());
    }

    #[test]
    fn resampling_preserves_length_in_time() {
        // Getting the ratio backwards halves or doubles every timestamp the
        // pipeline produces, and the transcript is then attributed to the
        // wrong moments throughout.
        let one_second_at_48k = vec![0.0_f32; 48_000];
        assert_eq!(
            resample_to_model_rate(&one_second_at_48k, 48_000).len(),
            SAMPLE_RATE as usize
        );
        let already = vec![0.0_f32; 16_000];
        assert_eq!(resample_to_model_rate(&already, SAMPLE_RATE).len(), 16_000);
        assert!(resample_to_model_rate(&[], 48_000).is_empty());
    }

    #[test]
    fn a_missing_audio_file_is_an_error_rather_than_a_panic() {
        // Meetings whose audio the Operator deleted are ordinary.
        assert!(decode(Path::new("/nonexistent/meeting.m4a")).is_err());
    }
}
