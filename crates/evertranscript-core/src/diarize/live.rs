//! The local ONNX pipeline: segmentation, embedding, clustering.
//!
//! Two models, both run through `ort`, both entirely on this machine —
//! story 33 forbids a cloud form in any shape, so there is no client, no
//! endpoint, and no fallback path to review.
//!
//! **The architecture: pyannote's, without its sliding overlap.**
//!
//!   1. Segmentation runs on consecutive 10 s chunks and says, per frame,
//!      which of up to three *local* speakers are talking — local because
//!      speaker 1 of one chunk has nothing to do with speaker 1 of the next.
//!   2. Each local speaker of each chunk is embedded from its own frames:
//!      the frames it holds alone where there are enough of them, and all
//!      of its frames otherwise ([`chosen_rows`]).
//!   3. [`super::cluster::agglomerate`] groups those embeddings into voices,
//!      which is what makes local speakers global.
//!   4. Every frame a local speaker was active is a turn of its voice
//!      ([`assemble`]). Two people talking at once are two turns that
//!      overlap, and a stretch is as short as the model says it is.
//!
//! **What the previous shape cost.** The first pipeline used segmentation
//! only for *how many* people were talking, kept the frames where that was
//! one, cut them into 3 s windows and clustered the windows. Overlapped
//! speech longer than the merge gap got no turn, and neither did a stretch
//! under 1.5 s; on the AMI test set (M3 ticket 09) that placed 27% of speech
//! with nobody even when every window was clustered perfectly. This shape
//! places speech where the segmentation model hears it and asks the
//! embedding only *whose* it is.
//!
//! Chunks do not overlap, where pyannote steps every second and averages
//! ten opinions of each frame. That is a tenth of the segmentation work
//! and a fraction of the embeddings for the cubic clusterer, and each voice
//! is still embedded from up to 10 s of itself. The seam at a chunk boundary
//! is closed by clustering rather than stitching: both halves of a turn
//! embed to the same voice, and `merge_adjacent` joins them.

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
use super::SampleWindow;
use super::Turn;
use super::fbank::FRAME_LENGTH;
use super::fbank::FRAME_SHIFT;
use super::fbank::MEL_BINS;
use super::fbank::MelBank;
use super::fbank::SAMPLE_RATE;

/// Window the segmentation model was trained on: 10 s at 16 kHz.
pub const SEGMENT_WINDOW: usize = 10 * SAMPLE_RATE as usize;

/// Powerset classes for three speakers: none, three singles, three pairs.
pub const POWERSET_CLASSES: usize = 7;

/// How many people one chunk can tell apart.
pub const LOCAL_SPEAKERS: usize = 3;

/// Which speakers each powerset class means are active.
///
/// Order is the model's, not ours. Getting this wrong produces a pipeline
/// that runs perfectly and mislabels every overlap — the exact class of
/// silent error the fbank module is also written to avoid.
const POWERSET: [&[usize]; POWERSET_CLASSES] = [&[], &[0], &[1], &[2], &[0, 1], &[0, 2], &[1, 2]];

/// Fewest filterbank frames the embedding model accepts.
///
/// Eight produce infinities; nine is what pyannote's `min_num_frames` works
/// out to for this ONNX. About 90 ms: a local speaker holding less than
/// that in a chunk is a cough or a boundary artefact, and gets no voice and
/// no turn.
pub const MIN_EMBED_FRAMES: usize = 9;

/// Shortest stretch of a voice on its own worth playing back.
///
/// The catalog's minimum voiced duration. Below this a sample says less
/// about a voice than no sample at all.
pub const MIN_SPAN_MS: u64 = 1_500;

/// Longest sample kept — the catalog's middle-10s clip.
pub const MAX_SPAN_MS: u64 = 10_000;

/// Gaps shorter than this do not end a turn (catalog: 400 ms merge gap).
pub const MERGE_GAP_MS: u64 = 400;

/// The embedding model, as every vector it produces is labelled.
pub const EMBEDDING_MODEL: &str = "wespeaker-voxceleb-resnet34-LM";

/// Which front end fed it.
///
/// "1" was the pipeline that shipped with M3, whose filterbank was not the
/// one the model was trained on (see `fbank`'s module note); "2" is the
/// corrected one. A "1" vector and a "2" vector of the same audio agree at
/// cosine 0.36 on average, so they must never be compared: seeds are read
/// by version, and a "1" exemplar is rebuilt from its kept sample before
/// the next Diarization reads seeds at all (`cluster::adopt_rebuilt`).
pub const EMBEDDING_MODEL_VERSION: &str = "2";

fn failed(error: impl std::fmt::Display) -> DiarizeError {
    DiarizeError::Failed(anyhow::anyhow!("{error}"))
}

fn open(path: &Path) -> Result<Session, DiarizeError> {
    Session::builder()
        .and_then(|mut builder| builder.commit_from_file(path))
        .map_err(|error| DiarizeError::Unavailable(format!("{}: {error}", path.display())))
}

/// The embedding model and its front end.
///
/// Its own type because two jobs need it and only one needs segmentation:
/// diarizing a Meeting, and rebuilding a Speaker's exemplars from kept
/// audio after the model or its front end changed.
pub struct Embedder {
    session: Session,
    mel: MelBank,
    frontend: Frontend,
}

/// Where the model's filterbank lives.
///
/// Two embeddings in this product's history want different things at their
/// input, and the difference is not a detail of either: WeSpeaker is handed
/// Kaldi features this crate computes, and ReDimNet2 is handed the waveform
/// and computes its own inside the graph. Keeping both selectable is what
/// lets the two be compared on one pipeline — the only way to attribute a
/// difference to the model rather than to everything else that moved with
/// it, which Q115 is the cautionary tale for: the bake-off that chose
/// between them ran every candidate through a front end that was wrong for
/// one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frontend {
    /// Kaldi fbank computed here, fed as `input_features`.
    Fbank,
    /// Raw 16 kHz waveform, fed as `waveform`; the graph owns the mel.
    Waveform,
}

impl Embedder {
    pub fn load(path: &Path) -> Result<Self, DiarizeError> {
        Self::load_with(path, Frontend::Fbank)
    }

    pub fn load_with(path: &Path, frontend: Frontend) -> Result<Self, DiarizeError> {
        Ok(Self {
            session: open(path)?,
            mel: MelBank::new(),
            frontend,
        })
    }

    pub fn frontend(&self) -> Frontend {
        self.frontend
    }

    /// Embeds a stretch of raw audio, for a model that carries its own mel.
    /// `None` when there is too little of it to run the model on.
    pub fn embed_samples(&mut self, samples: &[f32]) -> Result<Option<Vec<f32>>, DiarizeError> {
        if samples.len() < MIN_EMBED_FRAMES * FRAME_SHIFT {
            return Ok(None);
        }
        let input =
            Value::from_array(([1_usize, samples.len()], samples.to_vec())).map_err(failed)?;
        let outputs = self
            .session
            .run(ort::inputs!["waveform" => input])
            .map_err(failed)?;
        let (_, vector) = outputs["embedding"]
            .try_extract_tensor::<f32>()
            .map_err(failed)?;
        let mut vector = vector.to_vec();
        super::cluster::l2_normalize(&mut vector);
        Ok(Some(vector))
    }

    /// Log-mel features of a stretch of audio, mean-normalized over it.
    pub fn features(&self, samples: &[f32]) -> Vec<Vec<f32>> {
        self.mel.compute(samples)
    }

    /// Embeds a selection of feature frames — one voice's, not necessarily
    /// contiguous. `None` when there are too few to run the model on.
    pub fn embed_frames(&mut self, rows: &[&[f32]]) -> Result<Option<Vec<f32>>, DiarizeError> {
        if rows.len() < MIN_EMBED_FRAMES {
            return Ok(None);
        }
        let flat: Vec<f32> = rows.iter().flat_map(|row| row.iter().copied()).collect();
        let input = Value::from_array(([1_usize, rows.len(), MEL_BINS], flat)).map_err(failed)?;
        let outputs = self
            .session
            .run(ort::inputs!["input_features" => input])
            .map_err(failed)?;
        let (_, vector) = outputs["last_hidden_state"]
            .try_extract_tensor::<f32>()
            .map_err(failed)?;

        let mut vector = vector.to_vec();
        super::cluster::l2_normalize(&mut vector);
        Ok(Some(vector))
    }

    /// Embeds a stretch of audio whole: one voice, on its own.
    pub fn embed(&mut self, samples: &[f32]) -> Result<Option<Vec<f32>>, DiarizeError> {
        let features = self.features(samples);
        let rows: Vec<&[f32]> = features.iter().map(Vec::as_slice).collect();
        self.embed_frames(&rows)
    }
}

/// The live Diarizer.
pub struct LiveDiarizer {
    segmentation: Session,
    embedder: Embedder,
}

/// One local speaker of one chunk: what it said, where, and how it sounds.
///
/// The unit clustering works on. `cluster` is provisional — unique to this
/// observation — until [`super::cluster::agglomerate`] says which
/// observations are one voice.
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    pub channel: AudioChannel,
    pub cluster: Cluster,
    pub vector: Vec<f32>,
    /// Every stretch this speaker was active, on the capture clock.
    pub runs: Vec<(u64, u64)>,
    /// The stretches it was the only voice: where to play it back from.
    pub clean_runs: Vec<(u64, u64)>,
}

impl Observation {
    pub fn voiced_ms(&self) -> u64 {
        self.runs.iter().map(|(start, end)| end - start).sum()
    }
}

impl LiveDiarizer {
    /// Loads both models. Failure here is [`DiarizeError::Unavailable`] at
    /// the call site, never a lost Meeting.
    pub fn load(segmentation: &Path, embedding: &Path) -> Result<Self, DiarizeError> {
        Self::load_with(segmentation, embedding, Frontend::Fbank)
    }

    /// As [`load`](Self::load), choosing which front end the embedding wants.
    /// The measurement harness is the caller; production takes the default.
    pub fn load_with(
        segmentation: &Path,
        embedding: &Path,
        frontend: Frontend,
    ) -> Result<Self, DiarizeError> {
        Ok(Self {
            segmentation: open(segmentation)?,
            embedder: Embedder::load_with(embedding, frontend)?,
        })
    }

    pub fn embedder(&mut self) -> &mut Embedder {
        &mut self.embedder
    }

    /// Which local speakers are active in each frame of one chunk, as a
    /// bitmask over the three slots.
    fn segment(&mut self, chunk: &[f32]) -> Result<Vec<u8>, DiarizeError> {
        debug_assert_eq!(chunk.len(), SEGMENT_WINDOW);
        let input =
            Value::from_array(([1_usize, 1, SEGMENT_WINDOW], chunk.to_vec())).map_err(failed)?;
        let outputs = self
            .segmentation
            .run(ort::inputs!["input_values" => input])
            .map_err(failed)?;
        let (shape, logits) = outputs["logits"]
            .try_extract_tensor::<f32>()
            .map_err(failed)?;

        let frames = shape[1] as usize;
        let classes = shape[2] as usize;
        Ok((0..frames)
            .map(|frame| {
                let row = &logits[frame * classes..(frame + 1) * classes];
                let best = row
                    .iter()
                    .enumerate()
                    .max_by(|a, b| a.1.total_cmp(b.1))
                    .map(|(index, _)| index)
                    .unwrap_or(0);
                POWERSET
                    .get(best)
                    .map(|set| set.iter().fold(0_u8, |mask, speaker| mask | 1 << speaker))
                    .unwrap_or(0)
            })
            .collect())
    }

    /// Runs both models over a Meeting: every local speaker of every chunk,
    /// embedded, before anything is decided about who is who.
    pub fn observe(
        &mut self,
        audio: MeetingAudio<'_>,
        progress: &mut dyn FnMut(Progress),
        cancel: &Cancel,
    ) -> Result<Vec<Observation>, DiarizeError> {
        if audio.sample_rate != SAMPLE_RATE {
            return Err(DiarizeError::Unavailable(format!(
                "diarization needs {SAMPLE_RATE} Hz audio, was given {}",
                audio.sample_rate
            )));
        }

        let total_ms = audio.duration_ms();
        let mut observations = Vec::new();

        for (channel, samples) in [
            (AudioChannel::Mic, audio.mic),
            (AudioChannel::System, audio.system),
        ] {
            for start in (0..samples.len()).step_by(SEGMENT_WINDOW) {
                if cancel.is_cancelled() {
                    return Err(DiarizeError::Cancelled);
                }
                let end = (start + SEGMENT_WINDOW).min(samples.len());
                let mut chunk = vec![0.0_f32; SEGMENT_WINDOW];
                chunk[..end - start].copy_from_slice(&samples[start..end]);

                let masks = self.segment(&chunk)?;
                // Derived from the model's own output rather than assumed:
                // a hard-coded frame duration is how every timestamp in a
                // pipeline ends up scaled by a constant nobody notices.
                let samples_per_frame = SEGMENT_WINDOW as f64 / masks.len() as f64;
                let frame_ms = samples_per_frame * 1000.0 / SAMPLE_RATE as f64;
                // The padded tail of the last chunk is silence the model
                // was shown, not audio anyone spoke.
                let covered = ((end - start) as f64 / samples_per_frame) as usize;
                let masks = &masks[..covered.min(masks.len())];
                let chunk_start_ms = start as u64 * 1000 / SAMPLE_RATE as u64;
                let to_ms = |frame: usize| chunk_start_ms + (frame as f64 * frame_ms) as u64;

                // Features over the audio actually present, so the mean
                // that is subtracted is the mean of speech, not of speech
                // and padding.
                let features = self.embedder.features(&samples[start..end]);

                for local in 0..LOCAL_SPEAKERS {
                    let bit = 1_u8 << local;
                    let runs = runs_of(masks, |mask| mask & bit != 0);
                    if runs.is_empty() {
                        continue;
                    }
                    // The same selection either way — the frames this
                    // speaker holds alone where there are enough of them,
                    // all of its frames otherwise — expressed in whatever
                    // the model eats. Selecting differently per frontend
                    // would put a second variable next to the one being
                    // measured.
                    let vector = match self.embedder.frontend() {
                        Frontend::Fbank => {
                            let rows: Vec<&[f32]> = chosen_rows(masks, bit, samples_per_frame)
                                .into_iter()
                                .filter_map(|row| features.get(row).map(Vec::as_slice))
                                .collect();
                            self.embedder.embed_frames(&rows)?
                        }
                        Frontend::Waveform => {
                            let alone = runs_of(masks, |mask| mask == bit);
                            let picked = if alone.len() > 1
                                || alone.iter().map(|(a, b)| b - a).sum::<usize>()
                                    > MIN_EMBED_FRAMES
                            {
                                alone
                            } else {
                                runs_of(masks, |mask| mask & bit != 0)
                            };
                            let mut voiced: Vec<f32> = Vec::new();
                            for (from, to) in picked {
                                let a = start + (from as f64 * samples_per_frame) as usize;
                                let b = (start + (to as f64 * samples_per_frame) as usize)
                                    .min(samples.len());
                                if a < b {
                                    voiced.extend_from_slice(&samples[a..b]);
                                }
                            }
                            self.embedder.embed_samples(&voiced)?
                        }
                    };
                    let Some(vector) = vector else {
                        continue;
                    };
                    let in_ms = |runs: Vec<(usize, usize)>| -> Vec<(u64, u64)> {
                        runs.into_iter()
                            .map(|(from, to)| (to_ms(from), to_ms(to)))
                            .collect()
                    };
                    observations.push(Observation {
                        channel,
                        cluster: Cluster(observations.len() as u32),
                        vector,
                        runs: in_ms(runs),
                        clean_runs: in_ms(runs_of(masks, |mask| mask == bit)),
                    });
                }

                progress(Progress {
                    done_ms: to_ms(masks.len()).min(total_ms),
                    total_ms,
                });
            }
        }
        Ok(observations)
    }
}

/// Maximal stretches of frames that satisfy `keep`, as `[from, to)`.
pub fn runs_of(masks: &[u8], keep: impl Fn(u8) -> bool) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut open: Option<usize> = None;
    for (index, mask) in masks.iter().enumerate() {
        match (keep(*mask), open) {
            (true, None) => open = Some(index),
            (false, Some(from)) => {
                runs.push((from, index));
                open = None;
            }
            _ => {}
        }
    }
    if let Some(from) = open {
        runs.push((from, masks.len()));
    }
    runs
}

/// Which feature frames to embed one local speaker from.
///
/// pyannote's rule: the frames the speaker holds alone, unless there are
/// too few of them to embed, in which case all of its frames — overlap
/// included, because a voice heard only through crosstalk is still better
/// placed by a smudged vector than by none. Segmentation frames are ~17 ms
/// and feature frames 10 ms; each feature frame takes the segmentation
/// frame its centre falls in.
pub fn chosen_rows(masks: &[u8], bit: u8, samples_per_frame: f64) -> Vec<usize> {
    let rows = |keep: &dyn Fn(u8) -> bool| -> Vec<usize> {
        (0..)
            .map(|row| {
                (
                    row,
                    ((row * FRAME_SHIFT + FRAME_LENGTH / 2) as f64 / samples_per_frame) as usize,
                )
            })
            .take_while(|(_, frame)| *frame < masks.len())
            .filter(|(_, frame)| keep(masks[*frame]))
            .map(|(row, _)| row)
            .collect()
    };
    let alone = rows(&|mask| mask == bit);
    if alone.len() > MIN_EMBED_FRAMES {
        alone
    } else {
        rows(&|mask| mask & bit != 0)
    }
}

/// The sub-span of a clean stretch to hold a voice up by: its middle, at
/// most 10 s.
///
/// **A rule about samples, never about turns**, and that separation was a
/// measurement finding. The first version used these rules — the catalog's,
/// for *Voiceprint quality* — to define turns as well, and the close-out's
/// DER came back 38.4% with 28% of speech simply missed. Of course it did:
/// clipping a 15 s turn to its middle 10 s throws away a third of it, which
/// is exactly right for choosing what to play and exactly wrong for saying
/// who was talking.
///
/// The ends of a long stretch are where a neighbour's words bleed in, so
/// the middle. Returns `None` when there is too little to stand for a
/// voice — the voice still exists, it just has nowhere to be heard from.
pub fn embeddable(start_ms: u64, end_ms: u64) -> Option<(u64, u64)> {
    let length = end_ms.saturating_sub(start_ms);
    if length < MIN_SPAN_MS {
        return None;
    }
    if length <= MAX_SPAN_MS {
        return Some((start_ms, end_ms));
    }
    let middle = start_ms + length / 2;
    Some((middle - MAX_SPAN_MS / 2, middle + MAX_SPAN_MS / 2))
}

/// Joins consecutive turns of the same voice on the same channel.
fn merge_adjacent(mut turns: Vec<Turn>) -> Vec<Turn> {
    turns.sort_by_key(|turn| {
        (
            turn.channel == AudioChannel::System,
            turn.cluster,
            turn.start.millis(),
        )
    });
    let mut merged: Vec<Turn> = Vec::with_capacity(turns.len());
    for turn in turns {
        match merged.last_mut() {
            Some(previous)
                if previous.channel == turn.channel
                    && previous.cluster == turn.cluster
                    && turn.start.millis() <= previous.end.millis() + MERGE_GAP_MS =>
            {
                if turn.end > previous.end {
                    previous.end = turn.end;
                }
            }
            _ => merged.push(turn),
        }
    }
    merged
}

/// Turns observations and a clustering into a Diarization.
///
/// `canonical` maps each observation's provisional cluster to its voice;
/// an observation it does not mention is a voice of its own.
pub fn assemble(
    observations: &[Observation],
    canonical: &BTreeMap<Cluster, Cluster>,
) -> Diarization {
    let mut turns = Vec::new();
    let mut clean = Vec::new();
    let mut grouped: BTreeMap<Cluster, Vec<(Vec<f32>, i64, bool)>> = BTreeMap::new();
    for observation in observations {
        let voice = canonical
            .get(&observation.cluster)
            .copied()
            .unwrap_or(observation.cluster);
        let turn =
            |(start, end): &(u64, u64)| Turn::new(observation.channel, *start, *end, voice.index());
        turns.extend(observation.runs.iter().map(turn));
        clean.extend(observation.clean_runs.iter().map(turn));
        grouped.entry(voice).or_default().push((
            observation.vector.clone(),
            observation.voiced_ms() as i64,
            false,
        ));
    }

    // Consecutive frames, chunks and local speakers of one voice are one
    // turn. Without this the transcript would be attributed correctly and
    // read as a stutter, the same speaker restarting every ten seconds.
    let mut turns = merge_adjacent(turns);
    let clean = merge_adjacent(clean);

    // How much of each voice there was, measured on the merged turns so
    // that two local speakers of one voice in one chunk are not counted
    // twice; and where best to hear it — the middle of its longest stretch
    // alone.
    let mut voiced: BTreeMap<Cluster, u64> = BTreeMap::new();
    for turn in &turns {
        *voiced.entry(turn.cluster).or_default() += turn.duration_ms();
    }
    let mut longest: BTreeMap<Cluster, Turn> = BTreeMap::new();
    for turn in &clean {
        let best = longest.entry(turn.cluster).or_insert(*turn);
        if turn.duration_ms() > best.duration_ms() {
            *best = *turn;
        }
    }

    let embeddings = grouped
        .into_iter()
        .filter_map(|(cluster, observations)| {
            let vector = super::cluster::centroid(&observations)?;
            let mut embedding = Embedding::new(
                vector,
                EMBEDDING_MODEL,
                EMBEDDING_MODEL_VERSION,
                voiced.get(&cluster).copied().unwrap_or(0),
            );
            if let Some(turn) = longest.get(&cluster)
                && let Some((start, end)) = embeddable(turn.start.millis(), turn.end.millis())
            {
                embedding = embedding.with_sample(SampleWindow {
                    channel: turn.channel,
                    start: crate::audio::CaptureOffset(start),
                    end: crate::audio::CaptureOffset(end),
                });
            }
            Some((cluster, embedding))
        })
        .collect();

    turns.sort_by_key(|turn| (turn.start.millis(), turn.channel == AudioChannel::System));
    Diarization { turns, embeddings }
}

impl Diarizer for LiveDiarizer {
    fn diarize(
        &mut self,
        audio: MeetingAudio<'_>,
        progress: &mut dyn FnMut(Progress),
        cancel: &Cancel,
    ) -> Result<Diarization, DiarizeError> {
        let observations = self.observe(audio, progress, cancel)?;

        // Every observation starts as its own cluster; grouping them is
        // what turns local speakers into voices.
        let provisional: BTreeMap<Cluster, Embedding> = observations
            .iter()
            .map(|observation| {
                (
                    observation.cluster,
                    Embedding::new(
                        observation.vector.clone(),
                        EMBEDDING_MODEL,
                        EMBEDDING_MODEL_VERSION,
                        observation.voiced_ms(),
                    ),
                )
            })
            .collect();
        let canonical = super::cluster::agglomerate(&provisional);
        let result = assemble(&observations, &canonical);

        let total_ms = audio.duration_ms();
        progress(Progress {
            done_ms: total_ms,
            total_ms,
        });
        Ok(result)
    }

    fn describe(&self) -> String {
        format!("onnx diarizer ({EMBEDDING_MODEL})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One 10 s chunk's frame geometry, as the segmentation model has it.
    const SAMPLES_PER_FRAME: f64 = SEGMENT_WINDOW as f64 / 589.0;

    fn observation(
        channel: AudioChannel,
        cluster: u32,
        runs: &[(u64, u64)],
        clean: &[(u64, u64)],
    ) -> Observation {
        Observation {
            channel,
            cluster: Cluster(cluster),
            vector: vec![1.0, 0.0],
            runs: runs.to_vec(),
            clean_runs: clean.to_vec(),
        }
    }

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
    fn a_run_is_every_frame_a_speaker_holds_overlap_included() {
        // Speaker 0 alone, then with speaker 1, then speaker 1 alone. Two
        // people talking at once is two speakers, not nobody.
        let mut masks = vec![0b01_u8; 100];
        masks[60..80].fill(0b11);
        masks[80..100].fill(0b10);
        assert_eq!(runs_of(&masks, |mask| mask & 0b01 != 0), vec![(0, 80)]);
        assert_eq!(runs_of(&masks, |mask| mask & 0b10 != 0), vec![(60, 100)]);
        assert_eq!(runs_of(&masks, |mask| mask == 0b01), vec![(0, 60)], "alone");
        assert!(runs_of(&[], |_| true).is_empty());
    }

    #[test]
    fn a_voice_is_embedded_from_the_frames_it_holds_alone() {
        // 589 frames: speaker 0 alone for the first half, overlapped with
        // speaker 1 for the second. The feature rows chosen all land in
        // the first half.
        let mut masks = vec![0b01_u8; 589];
        masks[295..].fill(0b11);
        let rows = chosen_rows(&masks, 0b01, SAMPLES_PER_FRAME);
        assert!(
            rows.len() > 400,
            "about five seconds of 10 ms rows: {}",
            rows.len()
        );
        let last_centre = (rows.last().unwrap() * FRAME_SHIFT + FRAME_LENGTH / 2) as f64;
        assert!(
            last_centre < 295.0 * SAMPLES_PER_FRAME,
            "none from the overlap"
        );
    }

    #[test]
    fn a_voice_heard_only_in_overlap_is_embedded_from_all_of_its_frames() {
        // pyannote's fallback. Too few clean frames to embed, so every
        // frame the speaker holds is used — smudged beats absent.
        let mut masks = vec![0_u8; 589];
        masks[100..200].fill(0b11);
        masks[200..203].fill(0b10); // 3 frames alone: under the minimum
        let rows = chosen_rows(&masks, 0b10, SAMPLES_PER_FRAME);
        assert!(
            rows.len() > 150,
            "the overlapped stretch is used: {}",
            rows.len()
        );
    }

    #[test]
    fn overlapped_speech_is_a_turn_for_each_voice() {
        let observations = vec![
            observation(AudioChannel::Mic, 0, &[(0, 5_000)], &[(0, 3_000)]),
            observation(AudioChannel::Mic, 1, &[(3_000, 8_000)], &[(5_000, 8_000)]),
        ];
        let result = assemble(&observations, &BTreeMap::new());
        assert_eq!(result.turns.len(), 2);
        assert_eq!(result.turns[0].end.millis(), 5_000);
        assert_eq!(
            result.turns[1].start.millis(),
            3_000,
            "the overlap belongs to both"
        );
        assert_eq!(result.embeddings[&Cluster(0)].voiced_ms, 5_000);
    }

    #[test]
    fn a_short_stretch_is_still_a_turn() {
        // The defect the AMI measurement found: a 200 ms "yes" used to get
        // no speaker at all. It is a turn now; whether it also earns a
        // Speaker is `persist`'s question, not this one's.
        let observations = vec![observation(
            AudioChannel::System,
            0,
            &[(1_000, 1_200)],
            &[(1_000, 1_200)],
        )];
        let result = assemble(&observations, &BTreeMap::new());
        assert_eq!(result.turns.len(), 1);
        assert_eq!(result.turns[0].duration_ms(), 200);
        assert!(
            result.embeddings[&Cluster(0)].sample.is_none(),
            "too short to play"
        );
    }

    #[test]
    fn two_local_speakers_of_one_voice_are_one_turn() {
        // The same person in two consecutive chunks — local speaker 1,
        // then local speaker 0 — clustered together.
        let observations = vec![
            observation(AudioChannel::Mic, 0, &[(7_000, 10_000)], &[(7_000, 10_000)]),
            observation(
                AudioChannel::Mic,
                1,
                &[(10_000, 12_000)],
                &[(10_000, 12_000)],
            ),
        ];
        let canonical = [(Cluster(0), Cluster(0)), (Cluster(1), Cluster(0))]
            .into_iter()
            .collect();
        let result = assemble(&observations, &canonical);
        assert_eq!(result.turns.len(), 1);
        assert_eq!(
            (result.turns[0].start.millis(), result.turns[0].end.millis()),
            (7_000, 12_000)
        );
        assert_eq!(result.embeddings.len(), 1);
        assert_eq!(result.embeddings[&Cluster(0)].voiced_ms, 5_000);
        let sample = result.embeddings[&Cluster(0)].sample.expect("playable");
        assert_eq!(
            (sample.start.millis(), sample.end.millis()),
            (7_000, 12_000),
            "the clean stretch joined too"
        );
    }

    #[test]
    fn the_sample_is_taken_from_where_the_voice_is_alone() {
        let observations = vec![observation(
            AudioChannel::Mic,
            0,
            &[(0, 10_000)],
            &[(0, 1_000), (4_000, 7_000)],
        )];
        let result = assemble(&observations, &BTreeMap::new());
        let sample = result.embeddings[&Cluster(0)].sample.expect("playable");
        assert_eq!((sample.start.millis(), sample.end.millis()), (4_000, 7_000));
    }

    #[test]
    fn a_long_stretch_is_sampled_from_its_middle() {
        // The bug this guards against cost 28% of speech in the close-out
        // measurement: clipping the *turn* to the middle 10 s meant two
        // thirds of a long turn had no speaker at all.
        let (start, end) = embeddable(0, 60_000).expect("embeddable");
        assert_eq!(end - start, MAX_SPAN_MS);
        assert!(start > 20_000, "taken from the middle, not the start");
        assert_eq!(embeddable(0, 2_000), Some((0, 2_000)));
        assert_eq!(embeddable(0, 900), None);
    }

    #[test]
    fn consecutive_turns_of_one_voice_become_one_turn() {
        let merged = merge_adjacent(vec![
            Turn::new(AudioChannel::Mic, 0, 3_000, 0),
            Turn::new(AudioChannel::Mic, 1_500, 4_500, 0),
            Turn::new(AudioChannel::Mic, 4_700, 6_000, 0), // a 200 ms breath
        ]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].end.millis(), 6_000);
    }

    #[test]
    fn a_speaker_change_is_not_merged_away() {
        let merged = merge_adjacent(vec![
            Turn::new(AudioChannel::Mic, 0, 3_000, 0),
            Turn::new(AudioChannel::Mic, 1_500, 4_500, 1),
        ]);
        assert_eq!(merged.len(), 2, "two voices stay two turns");
    }

    #[test]
    fn the_two_channels_never_merge_into_each_other() {
        // The room and the far end are different people by construction.
        let merged = merge_adjacent(vec![
            Turn::new(AudioChannel::Mic, 0, 3_000, 0),
            Turn::new(AudioChannel::System, 1_500, 4_500, 0),
        ]);
        assert_eq!(merged.len(), 2);
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

    fn load(dir: &Path) -> LiveDiarizer {
        LiveDiarizer::load(&dir.join("segmentation.onnx"), &dir.join("embedding.onnx"))
            .expect("both models load")
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
        let mut diarizer = load(&dir);

        let speech = tone(140.0, 3.0, 6);
        let vector = diarizer
            .embedder()
            .embed(&speech)
            .expect("embeds")
            .expect("a vector");
        assert_eq!(vector.len(), 256, "the embedding model's stated width");
        let norm: f32 = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3, "L2-normalized, got {norm}");

        let too_short: Vec<f32> = speech[..FRAME_LENGTH + 7 * FRAME_SHIFT].to_vec();
        assert!(
            diarizer
                .embedder()
                .embed(&too_short)
                .expect("runs")
                .is_none(),
            "eight frames is too few"
        );
    }

    #[test]
    fn segmentation_tells_speech_from_silence() {
        let Some(dir) = model_dir() else {
            eprintln!("skipped: set EVERTRANSCRIPT_DIARIZE_MODELS to run this");
            return;
        };
        let mut diarizer = load(&dir);
        let silent = diarizer
            .segment(&vec![0.0_f32; SEGMENT_WINDOW])
            .expect("runs on silence");
        assert!(
            silent.iter().all(|mask| *mask == 0),
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
        let mut diarizer = load(&dir);

        let low = diarizer
            .embedder()
            .embed(&tone(110.0, 3.0, 8))
            .unwrap()
            .unwrap();
        let high = diarizer
            .embedder()
            .embed(&tone(230.0, 3.0, 3))
            .unwrap()
            .unwrap();
        let same = diarizer
            .embedder()
            .embed(&tone(110.0, 3.0, 8))
            .unwrap()
            .unwrap();

        let across = super::super::cluster::cosine(&low, &high);
        let within = super::super::cluster::cosine(&low, &same);
        assert!(within > 0.99, "the same input must embed identically");
        assert!(
            within > across,
            "different signals must be further apart: within {within}, across {across}"
        );
    }

    #[test]
    fn a_whole_meeting_diarizes_end_to_end() {
        // The only test here that exercises `diarize` rather than a piece
        // of it. Every defect this project has shipped lived in a path
        // nothing executed.
        let Some(dir) = model_dir() else {
            eprintln!("skipped: set EVERTRANSCRIPT_DIARIZE_MODELS to run this");
            return;
        };
        let mut diarizer = load(&dir);

        // Two acoustically distinct signals, one after the other.
        let mut audio = tone(120.0, 6.0, 8);
        audio.extend(tone(260.0, 6.0, 3));
        let empty: Vec<f32> = Vec::new();

        let result = diarizer
            .diarize(
                MeetingAudio {
                    mic: &audio,
                    system: &empty,
                    sample_rate: SAMPLE_RATE,
                },
                &mut |_| {},
                &Cancel::new(),
            )
            .expect("diarizes without panicking");

        // Whatever it concludes about how many voices there are, the
        // structure has to be coherent: every turn's cluster has to be a
        // voice, and the embeddings have to describe the turns.
        for turn in &result.turns {
            assert!(turn.end > turn.start, "a turn with no duration: {turn:?}");
            assert!(
                turn.end.millis() <= 12_000,
                "inside the recording: {turn:?}"
            );
        }
        for cluster in result.clusters() {
            assert!(
                result.embeddings.contains_key(&cluster),
                "cluster {cluster:?} has turns but no voice"
            );
        }
        for (cluster, embedding) in &result.embeddings {
            assert_eq!(embedding.model_version, EMBEDDING_MODEL_VERSION);
            let spoken: u64 = result
                .turns
                .iter()
                .filter(|t| t.cluster == *cluster)
                .map(|t| t.duration_ms())
                .sum();
            assert_eq!(
                embedding.voiced_ms, spoken,
                "the voice knows its own length"
            );
            if let Some(sample) = embedding.sample {
                assert!(
                    result.turns.iter().any(|t| t.cluster == *cluster
                        && t.channel == sample.channel
                        && t.start <= sample.start
                        && sample.end <= t.end),
                    "the sample lies inside one of the voice's own turns"
                );
            }
        }
    }
}
