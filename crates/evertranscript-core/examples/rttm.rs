//! Diarizes one WAV file with the shipped pipeline and prints RTTM, so the
//! result can be scored against a reference (the M3 close-out's DER
//! measurement, DECISIONS Q111). Reproducible on purpose: the first harness
//! lived in a scratchpad and was deleted with it.
//!
//! ```text
//! EVERTRANSCRIPT_DIARIZE_MODELS=<dir with segmentation.onnx + embedding.onnx> \
//!   cargo run --release -p evertranscript-core --example rttm -- <file.wav> [uri]
//! ```
//!
//! A mono file is one in-room channel: it is fed as the microphone leg with
//! an empty system leg, which is how a meeting recorded in a room arrives.
//! A stereo file is a captured Meeting: left is the microphone, right the
//! system. Audio at another rate is linearly resampled to 16 kHz.
//!
//! With `EVERTRANSCRIPT_RTTM_DUMP=<file>` every observation the models made
//! — one local speaker of one chunk, with its vector and its frames — is
//! also written as JSON lines, before clustering. That is what an oracle
//! (perfect clustering) and a merge-threshold sweep re-cluster from, so both
//! score the same turns the product would place.

use std::path::PathBuf;
use std::time::Instant;

use std::collections::BTreeMap;
use std::io::Write;

use evertranscript_core::diarize::Cancel;
use evertranscript_core::diarize::Cluster;
use evertranscript_core::diarize::Embedding;
use evertranscript_core::diarize::MeetingAudio;
use evertranscript_core::diarize::cluster::agglomerate;
use evertranscript_core::diarize::fbank::SAMPLE_RATE;
use evertranscript_core::diarize::live::LiveDiarizer;
use evertranscript_core::diarize::live::assemble;
use evertranscript_core::diarize::live::provisional_of;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: rttm <file.wav> [uri]");
        std::process::exit(2);
    };
    let uri = args.next().unwrap_or_else(|| {
        PathBuf::from(&path)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "meeting".to_string())
    });
    let models = PathBuf::from(
        std::env::var("EVERTRANSCRIPT_DIARIZE_MODELS")
            .expect("EVERTRANSCRIPT_DIARIZE_MODELS must point at the model directory"),
    );

    let mut reader = hound::WavReader::open(&path).expect("open wav");
    let spec = reader.spec();
    let channels = spec.channels as usize;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .map(|s| s.expect("sample"))
            .collect(),
        hound::SampleFormat::Int => {
            let scale = (1_i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.expect("sample") as f32 / scale)
                .collect()
        }
    };
    let mut mic: Vec<f32> = samples.iter().step_by(channels).copied().collect();
    let mut system: Vec<f32> = if channels > 1 {
        samples.iter().skip(1).step_by(channels).copied().collect()
    } else {
        Vec::new()
    };
    if spec.sample_rate != SAMPLE_RATE {
        mic = resample(&mic, spec.sample_rate);
        system = resample(&system, spec.sample_rate);
    }

    let mut diarizer = LiveDiarizer::load(
        &models.join("segmentation.onnx"),
        &models.join("embedding.onnx"),
    )
    .expect("models load");
    let started = Instant::now();
    let observed = diarizer
        .observe(
            MeetingAudio {
                mic: &mic,
                system: &system,
                sample_rate: SAMPLE_RATE,
            },
            &mut |_| {},
            &Cancel::new(),
        )
        .expect("observes");
    let observations = &observed.observations;
    if let Ok(dump) = std::env::var("EVERTRANSCRIPT_RTTM_DUMP") {
        let mut file = std::fs::File::create(&dump).expect("create dump");
        for observation in observations {
            let line = serde_json::json!({
                "channel": observation.channel.as_str(),
                "runs": observation.runs,
                "clean": observation.clean_runs,
                "vector": observation.vector,
            });
            writeln!(file, "{line}").expect("write dump");
        }
    }
    // The one place the stamp is applied, rather than a copy of it here
    // that would go on naming the old model after a swap.
    let provisional: BTreeMap<Cluster, Embedding> = provisional_of(&observed);
    let result = assemble(&observed, &agglomerate(&provisional));
    eprintln!(
        "{uri}: {:.0} s of audio, {} observations, {} turns, {} voices, {} with a Voiceprint, in {:.1} s",
        mic.len() as f64 / SAMPLE_RATE as f64,
        observations.len(),
        result.turns.len(),
        result.clusters().len(),
        result.embeddings.len(),
        started.elapsed().as_secs_f64()
    );

    let mut out = String::new();
    for turn in &result.turns {
        let start = turn.start.millis() as f64 / 1000.0;
        let duration = turn.duration_ms() as f64 / 1000.0;
        let leg = if turn.channel == evertranscript_protocol::AudioChannel::Mic {
            "mic"
        } else {
            "sys"
        };
        out.push_str(&format!(
            "SPEAKER {uri} 1 {start:.3} {duration:.3} <NA> <NA> {leg}-c{} <NA> <NA>\n",
            turn.cluster.index()
        ));
    }
    print!("{out}");
}

fn resample(samples: &[f32], from_rate: u32) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }
    let ratio = SAMPLE_RATE as f64 / from_rate as f64;
    let count = (samples.len() as f64 * ratio) as usize;
    (0..count)
        .map(|index| {
            let source = index as f64 / ratio;
            let left = (source.floor() as usize).min(samples.len() - 1);
            let right = (left + 1).min(samples.len() - 1);
            let fraction = (source - left as f64) as f32;
            samples[left] * (1.0 - fraction) + samples[right] * fraction
        })
        .collect()
}
