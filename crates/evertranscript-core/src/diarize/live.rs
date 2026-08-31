//! The local ONNX pipeline: segmentation, embedding, clustering.
//!
//! Two models, both run through `ort`, both entirely on this machine —
//! story 33 forbids a cloud form in any shape, so there is no client, no
//! endpoint, and no fallback path to review.
//!
//! **The architecture, and one deliberate simplification.** The catalog
//! describes pyannote's full pipeline: sliding windows, per-window *local*
//! speaker labels, and a stitching step that reconciles those labels across
//! overlapping windows. That stitching is the hardest and least testable
//! part of the design, and it exists to recover speaker identity *within*
//! segmentation. This implementation instead uses segmentation for what it
//! is unambiguously good at — where speech is, and where two people overlap
//! — and recovers identity from embeddings afterwards:
//!
//!   1. Segmentation gives per-frame speaker counts over the whole channel.
//!   2. Contiguous speech becomes candidate spans, split at overlap edges.
//!   3. Each span is embedded.
//!   4. [`super::cluster::agglomerate`] groups spans into voices.
//!
//! That is a weaker treatment of overlapped speech than full stitching, and
//! it is written down here rather than discovered later: where two people
//! talk at once, this produces one turn attributed to whoever the embedding
//! resembles, not two. The close-out's DER is what says whether that trade
//! is acceptable, which is the point of owing a measurement.

use std::collections::BTreeMap;
use std::path::Path;

use evertranscript_protocol::AudioChannel;
use ort::session::Session;
use ort::value::Value;

use super::Cancel;
use super::Cluster;
use super::Diarization;
use super::DiarizeError;
use super::Diarizer;
use super::Embedding;
use super::MeetingAudio;
use super::Progress;
use super::Turn;
use super::fbank::MelBank;
use super::fbank::SAMPLE_RATE;

/// Window the segmentation model was trained on: 10 s at 16 kHz.
pub const SEGMENT_WINDOW: usize = 10 * SAMPLE_RATE as usize;

/// Powerset classes for three speakers: none, three singles, three pairs.
pub const POWERSET_CLASSES: usize = 7;

/// Which speakers each powerset class means are active.
///
/// Order is the model's, not ours. Getting this wrong produces a pipeline
/// that runs perfectly and mislabels every overlap — the exact class of
/// silent error the fbank module is also written to avoid.
const POWERSET: [&[usize]; POWERSET_CLASSES] = [&[], &[0], &[1], &[2], &[0, 1], &[0, 2], &[1, 2]];

/// Shortest span worth embedding.
///
/// The catalog's minimum voiced duration. Below this a Voiceprint is built
/// from too little evidence to be worth more than no Voiceprint at all.
pub const MIN_SPAN_MS: u64 = 1_500;

/// Longest span fed to the embedding model — the catalog's middle-10s clip.
pub const MAX_SPAN_MS: u64 = 10_000;

/// Gaps shorter than this do not end a span (catalog: 400 ms merge gap).
pub const MERGE_GAP_MS: u64 = 400;

/// The live Diarizer.
pub struct LiveDiarizer {
    segmentation: Session,
    embedding: Session,
    mel: MelBank,
    model_name: String,
    model_version: String,
}

impl LiveDiarizer {
    /// Loads both models. Failure here is [`DiarizeError::Unavailable`] at
    /// the call site, never a lost Meeting.
    pub fn load(segmentation: &Path, embedding: &Path) -> Result<Self, DiarizeError> {
        let open = |path: &Path| -> Result<Session, DiarizeError> {
            Session::builder()
                .and_then(|mut builder| builder.commit_from_file(path))
                .map_err(|error| DiarizeError::Unavailable(format!("{}: {error}", path.display())))
        };
        Ok(Self {
            segmentation: open(segmentation)?,
            embedding: open(embedding)?,
            mel: MelBank::new(),
            model_name: "wespeaker-voxceleb-resnet34-LM".to_string(),
            model_version: "1".to_string(),
        })
    }

    /// Per-frame speaker count for one channel, and the frame duration.
    fn speech_frames(&mut self, samples: &[f32]) -> Result<(Vec<usize>, f64), DiarizeError> {
        let mut counts: Vec<usize> = Vec::new();
        let mut frame_ms = 0.0_f64;

        for start in (0..samples.len()).step_by(SEGMENT_WINDOW) {
            let end = (start + SEGMENT_WINDOW).min(samples.len());
            let mut window = vec![0.0_f32; SEGMENT_WINDOW];
            window[..end - start].copy_from_slice(&samples[start..end]);

            let input = Value::from_array(([1_usize, 1, SEGMENT_WINDOW], window))
                .map_err(|error| DiarizeError::Failed(anyhow::anyhow!("{error}")))?;
            let outputs = self
                .segmentation
                .run(ort::inputs!["input_values" => input])
                .map_err(|error| DiarizeError::Failed(anyhow::anyhow!("{error}")))?;
            let (shape, logits) = outputs["logits"]
                .try_extract_tensor::<f32>()
                .map_err(|error| DiarizeError::Failed(anyhow::anyhow!("{error}")))?;

            let frames = shape[1] as usize;
            let classes = shape[2] as usize;
            if frame_ms == 0.0 {
                // Derived from the model's own output rather than assumed:
                // a hard-coded frame duration is how every timestamp in a
                // pipeline ends up scaled by a constant nobody notices.
                frame_ms = (SEGMENT_WINDOW as f64 / SAMPLE_RATE as f64) * 1000.0 / frames as f64;
            }

            let covered = ((end - start) as f64 / SEGMENT_WINDOW as f64 * frames as f64) as usize;
            for frame in 0..frames.min(covered) {
                let row = &logits[frame * classes..(frame + 1) * classes];
                let best = row
                    .iter()
                    .enumerate()
                    .max_by(|a, b| a.1.total_cmp(b.1))
                    .map(|(index, _)| index)
                    .unwrap_or(0);
                counts.push(POWERSET.get(best).map(|set| set.len()).unwrap_or(0));
            }
        }
        Ok((counts, frame_ms))
    }

    /// Embeds one span of audio.
    fn embed(&mut self, samples: &[f32]) -> Result<Option<Vec<f32>>, DiarizeError> {
        let features = self.mel.compute(samples);
        if features.is_empty() {
            return Ok(None);
        }
        let frames = features.len();
        let flat: Vec<f32> = features.into_iter().flatten().collect();

        let input = Value::from_array(([1_usize, frames, super::fbank::MEL_BINS], flat))
            .map_err(|error| DiarizeError::Failed(anyhow::anyhow!("{error}")))?;
        let outputs = self
            .embedding
            .run(ort::inputs!["input_features" => input])
            .map_err(|error| DiarizeError::Failed(anyhow::anyhow!("{error}")))?;
        let (_, vector) = outputs["last_hidden_state"]
            .try_extract_tensor::<f32>()
            .map_err(|error| DiarizeError::Failed(anyhow::anyhow!("{error}")))?;

        let mut vector = vector.to_vec();
        super::cluster::l2_normalize(&mut vector);
        Ok(Some(vector))
    }
}

/// Contiguous speech, as `(start_ms, end_ms)`.
///
/// Split out as a free function because it is the part worth testing without
/// a model: the span rules are the catalog's, and they decide what a
/// Voiceprint is ever built from.
pub fn spans(counts: &[usize], frame_ms: f64) -> Vec<(u64, u64)> {
    let mut spans: Vec<(u64, u64)> = Vec::new();
    let mut open: Option<u64> = None;

    for (index, count) in counts.iter().enumerate() {
        let at = (index as f64 * frame_ms) as u64;
        // Overlapped speech is excluded entirely, not merely marked: a
        // Voiceprint built from two people talking at once belongs to
        // neither of them (catalog: subtract overlapped same-channel
        // speech).
        let speaking = *count == 1;
        match (speaking, open) {
            (true, None) => open = Some(at),
            (false, Some(start)) => {
                spans.push((start, at));
                open = None;
            }
            _ => {}
        }
    }
    if let Some(start) = open {
        spans.push((start, (counts.len() as f64 * frame_ms) as u64));
    }

    // Merge across short gaps, then drop what is too short to mean anything.
    let mut merged: Vec<(u64, u64)> = Vec::new();
    for (start, end) in spans {
        match merged.last_mut() {
            Some(previous) if start.saturating_sub(previous.1) <= MERGE_GAP_MS => {
                previous.1 = end;
            }
            _ => merged.push((start, end)),
        }
    }
    merged
        .into_iter()
        .filter(|(start, end)| end.saturating_sub(*start) >= MIN_SPAN_MS)
        .map(|(start, end)| {
            // Clip to the middle of a long span: the beginning and end of a
            // turn are where a neighbour's words bleed in.
            let length = end - start;
            if length <= MAX_SPAN_MS {
                (start, end)
            } else {
                let middle = start + length / 2;
                (middle - MAX_SPAN_MS / 2, middle + MAX_SPAN_MS / 2)
            }
        })
        .collect()
}

impl Diarizer for LiveDiarizer {
    fn diarize(
        &mut self,
        audio: MeetingAudio<'_>,
        progress: &mut dyn FnMut(Progress),
        cancel: &Cancel,
    ) -> Result<Diarization, DiarizeError> {
        if audio.sample_rate != SAMPLE_RATE {
            return Err(DiarizeError::Unavailable(format!(
                "diarization needs {SAMPLE_RATE} Hz audio, was given {}",
                audio.sample_rate
            )));
        }

        let total_ms = audio.duration_ms();
        let mut turns = Vec::new();
        let mut vectors: Vec<(usize, Vec<f32>)> = Vec::new();
        let mut next_cluster = 0_u32;

        for (channel, samples) in [
            (AudioChannel::Mic, audio.mic),
            (AudioChannel::System, audio.system),
        ] {
            if cancel.is_cancelled() {
                return Err(DiarizeError::Cancelled);
            }
            if samples.is_empty() {
                continue;
            }

            let (counts, frame_ms) = self.speech_frames(samples)?;
            for (start_ms, end_ms) in spans(&counts, frame_ms) {
                if cancel.is_cancelled() {
                    return Err(DiarizeError::Cancelled);
                }
                let from = (start_ms as usize * SAMPLE_RATE as usize / 1000).min(samples.len());
                let to = (end_ms as usize * SAMPLE_RATE as usize / 1000).min(samples.len());
                if to <= from {
                    continue;
                }
                let Some(vector) = self.embed(&samples[from..to])? else {
                    continue;
                };

                let index = turns.len();
                turns.push(Turn::new(channel, start_ms, end_ms, next_cluster));
                vectors.push((index, vector));
                next_cluster += 1;

                progress(Progress {
                    done_ms: end_ms.min(total_ms),
                    total_ms,
                });
            }
        }

        // Every span started as its own cluster; grouping them is what turns
        // spans into voices.
        let provisional: BTreeMap<Cluster, Embedding> = vectors
            .iter()
            .map(|(index, vector)| {
                (
                    Cluster(turns[*index].cluster.index()),
                    Embedding::new(vector.clone(), &self.model_name, &self.model_version),
                )
            })
            .collect();
        let canonical = super::cluster::agglomerate(&provisional);

        for turn in turns.iter_mut() {
            if let Some(target) = canonical.get(&turn.cluster) {
                turn.cluster = *target;
            }
        }

        // One embedding per surviving cluster, averaged over its spans.
        let mut grouped: BTreeMap<Cluster, Vec<(Vec<f32>, i64, bool)>> = BTreeMap::new();
        for (index, vector) in vectors {
            let turn = turns[index];
            grouped.entry(turn.cluster).or_default().push((
                vector,
                turn.duration_ms() as i64,
                false,
            ));
        }
        let embeddings = grouped
            .into_iter()
            .filter_map(|(cluster, observations)| {
                super::cluster::centroid(&observations).map(|vector| {
                    (
                        cluster,
                        Embedding::new(vector, &self.model_name, &self.model_version),
                    )
                })
            })
            .collect();

        turns.sort_by_key(|turn| (turn.start.millis(), turn.channel == AudioChannel::System));
        progress(Progress {
            done_ms: total_ms,
            total_ms,
        });
        Ok(Diarization { turns, embeddings })
    }

    fn describe(&self) -> String {
        format!("onnx diarizer ({})", self.model_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_powerset_covers_three_speakers_and_their_pairs() {
        // Seven classes: silence, three singles, three pairs. If this table
        // ever disagrees with the model, every overlap is mislabelled and
        // nothing reports it.
        assert_eq!(POWERSET.len(), POWERSET_CLASSES);
        assert_eq!(POWERSET.iter().filter(|set| set.len() == 1).count(), 3);
        assert_eq!(POWERSET.iter().filter(|set| set.len() == 2).count(), 3);
        assert_eq!(POWERSET[0].len(), 0);
    }

    #[test]
    fn overlapped_speech_is_excluded_from_spans() {
        // A Voiceprint built from two people talking at once belongs to
        // neither of them.
        let frame_ms = 10.0;
        let mut counts = vec![1_usize; 400];
        counts[150..250].fill(2);
        let found = spans(&counts, frame_ms);
        assert!(
            found
                .iter()
                .all(|(start, end)| *end <= 1_500 || *start >= 2_500),
            "no span crosses the overlap: {found:?}"
        );
    }

    #[test]
    fn short_gaps_do_not_split_a_turn() {
        // Someone drawing breath is not the end of their turn.
        let frame_ms = 10.0;
        let mut counts = vec![1_usize; 600];
        counts[300..320].fill(0); // 200 ms, under the merge gap
        assert_eq!(spans(&counts, frame_ms).len(), 1);
    }

    #[test]
    fn a_long_gap_does_split_a_turn() {
        let frame_ms = 10.0;
        let mut counts = vec![1_usize; 800];
        counts[300..400].fill(0); // 1 s, well over the gap
        assert_eq!(spans(&counts, frame_ms).len(), 2);
    }

    #[test]
    fn spans_shorter_than_the_minimum_are_dropped() {
        // Not silently lost speech: the turn still exists upstream, it just
        // does not get to define a voice.
        let frame_ms = 10.0;
        let mut counts = vec![0_usize; 400];
        counts[0..50].fill(1); // 500 ms
        assert!(spans(&counts, frame_ms).is_empty());
    }

    #[test]
    fn a_very_long_span_is_clipped_to_its_middle() {
        // The beginning and end of a long turn are where a neighbour's words
        // bleed in.
        let frame_ms = 10.0;
        let counts = vec![1_usize; 6_000]; // 60 s
        let found = spans(&counts, frame_ms);
        assert_eq!(found.len(), 1);
        let (start, end) = found[0];
        assert_eq!(end - start, MAX_SPAN_MS);
        assert!(start > 20_000, "taken from the middle, not the start");
    }

    #[test]
    fn silence_produces_no_spans() {
        assert!(spans(&vec![0_usize; 1_000], 10.0).is_empty());
        assert!(spans(&[], 10.0).is_empty());
    }

    // ---- The tests that need the real models ----
    //
    // Skipped when the models are absent, which is the honest arrangement:
    // a CI job with no model files must not silently claim to have proven
    // inference works. `EVERTRANSCRIPT_DIARIZE_MODELS` points at a directory
    // holding `segmentation.onnx` and `embedding.onnx`.

    fn model_dir() -> Option<std::path::PathBuf> {
        let dir = std::env::var("EVERTRANSCRIPT_DIARIZE_MODELS").ok()?;
        let dir = std::path::PathBuf::from(dir);
        (dir.join("segmentation.onnx").exists() && dir.join("embedding.onnx").exists())
            .then_some(dir)
    }

    fn tone(hz: f32, seconds: f32, harmonics: usize) -> Vec<f32> {
        let count = (SAMPLE_RATE as f32 * seconds) as usize;
        (0..count)
            .map(|index| {
                let t = index as f32 / SAMPLE_RATE as f32;
                (1..=harmonics)
                    .map(|h| (2.0 * std::f32::consts::PI * hz * h as f32 * t).sin() / h as f32)
                    .sum::<f32>()
                    * 0.3
            })
            .collect()
    }

    #[test]
    fn the_models_load_and_actually_run() {
        // The M2 lesson, applied. Every Windows defect that milestone found
        // was code that compiled and had never executed. A diarizer that
        // links `ort` and has never run a tensor through it is in exactly
        // that state.
        let Some(dir) = model_dir() else {
            eprintln!("skipped: set EVERTRANSCRIPT_DIARIZE_MODELS to run this");
            return;
        };
        let mut diarizer =
            LiveDiarizer::load(&dir.join("segmentation.onnx"), &dir.join("embedding.onnx"))
                .expect("both models load");

        let speech = tone(140.0, 3.0, 6);
        let vector = diarizer.embed(&speech).expect("embeds").expect("a vector");
        assert_eq!(vector.len(), 256, "the embedding model's stated width");
        let norm: f32 = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3, "L2-normalized, got {norm}");
    }

    #[test]
    fn segmentation_tells_speech_from_silence() {
        let Some(dir) = model_dir() else {
            eprintln!("skipped: set EVERTRANSCRIPT_DIARIZE_MODELS to run this");
            return;
        };
        let mut diarizer =
            LiveDiarizer::load(&dir.join("segmentation.onnx"), &dir.join("embedding.onnx"))
                .expect("models load");

        let (silent, _) = diarizer
            .speech_frames(&vec![0.0_f32; SEGMENT_WINDOW])
            .expect("runs on silence");
        assert!(
            silent.iter().all(|count| *count == 0),
            "silence must contain no speakers"
        );
    }

    #[test]
    fn two_different_voices_embed_differently() {
        // The property the whole pipeline rests on. If two clearly different
        // signals produce near-identical vectors, clustering cannot work and
        // no threshold will save it.
        let Some(dir) = model_dir() else {
            eprintln!("skipped: set EVERTRANSCRIPT_DIARIZE_MODELS to run this");
            return;
        };
        let mut diarizer =
            LiveDiarizer::load(&dir.join("segmentation.onnx"), &dir.join("embedding.onnx"))
                .expect("models load");

        let low = diarizer.embed(&tone(110.0, 3.0, 8)).unwrap().unwrap();
        let high = diarizer.embed(&tone(230.0, 3.0, 3)).unwrap().unwrap();
        let same = diarizer.embed(&tone(110.0, 3.0, 8)).unwrap().unwrap();

        let across = super::super::cluster::cosine(&low, &high);
        let within = super::super::cluster::cosine(&low, &same);
        assert!(within > 0.99, "the same input must embed identically");
        assert!(
            within > across,
            "different signals must be further apart: within {within}, across {across}"
        );
    }
}
