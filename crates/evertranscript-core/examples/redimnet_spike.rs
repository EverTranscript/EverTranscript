//! Spike: does a ReDimNet2 ONNX export, mel frontend and STFT node included,
//! run through `ort` on this platform, and does it agree with PyTorch?
//!
//!     cargo run --release -p evertranscript-core --example redimnet_spike -- \
//!         <model.onnx> <reference.json>
//!
//! `reference.json` is the `.export.json` the export script writes; its
//! `reference_3s_first8` is compared here after the same fixed waveform is
//! rebuilt in Rust. The full 192-d comparison happens in Python against the
//! `.npy`; this binary proves the operator set loads and executes here, which
//! is the fact the design hangs on (DECISIONS: ReDimNet spike).

use std::path::PathBuf;

use ort::session::Session;
use ort::value::Value;

const SAMPLE_RATE: usize = 16_000;
const EMBED_DIM: usize = 192;

/// The export script's `fixed_waveform`: torch's seeded normal noise cannot
/// be reproduced here, so the Rust check uses the tones alone and compares
/// against a tone-only reference the script also writes.
fn tones(seconds: f32) -> Vec<f32> {
    let n = (seconds * SAMPLE_RATE as f32) as usize;
    (0..n)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            0.3 * (2.0 * std::f32::consts::PI * 220.0 * t).sin()
                + 0.2 * (2.0 * std::f32::consts::PI * 1375.0 * t).sin()
        })
        .collect()
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (na * nb)
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let model: PathBuf = args.next().expect("model.onnx").into();
    let reference: PathBuf = args.next().expect("reference.json").into();

    let started = std::time::Instant::now();
    let mut session = Session::builder()?.commit_from_file(&model)?;
    let loaded = started.elapsed();

    let inputs: Vec<String> = session
        .inputs()
        .iter()
        .map(|i| i.name().to_string())
        .collect();
    let outputs: Vec<String> = session
        .outputs()
        .iter()
        .map(|o| o.name().to_string())
        .collect();
    println!("inputs={inputs:?} outputs={outputs:?} load={loaded:?}");

    let report: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&reference)?)?;
    let expected: Vec<f32> = report["tones_only_3s"]
        .as_array()
        .expect("tones_only_3s")
        .iter()
        .map(|v| v.as_f64().unwrap() as f32)
        .collect();

    for seconds in [3.0_f32, 6.0] {
        let wave = tones(seconds);
        let len = wave.len();
        let input = Value::from_array(([1_usize, len], wave))?;
        let run_started = std::time::Instant::now();
        let out = session.run(ort::inputs!["waveform" => input])?;
        let elapsed = run_started.elapsed();
        let (shape, data) = out["embedding"].try_extract_tensor::<f32>()?;
        anyhow::ensure!(shape[1] as usize == EMBED_DIM, "bad shape {shape:?}");
        let first8: Vec<f32> = data[..8].to_vec();
        if seconds == 3.0 {
            let c = cosine(&data[..EMBED_DIM], &expected);
            println!("3s: cosine vs torch = {c:.6} run={elapsed:?} first8={first8:?}");
            anyhow::ensure!(c > 0.9999, "MISMATCH: cosine {c}");
        } else {
            println!("{seconds}s: shape={shape:?} run={elapsed:?} first8={first8:?}");
        }
    }
    println!("OK");
    Ok(())
}
