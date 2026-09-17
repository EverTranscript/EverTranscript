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

use std::collections::{BTreeMap, BTreeSet};
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
use crate::models::registry::VoiceprintId;

/// Window the segmentation model was trained on: 10 s at 16 kHz.
pub const SEGMENT_WINDOW: usize = 10 * SAMPLE_RATE as usize;

/// How far the window advances between segmentation runs.
///
/// A full window, so windows tile the recording and every instant is seen
/// once. pyannote's own recipe advances a tenth of a window and votes on
/// the overlap, and its published 18.8% DER is measured that way — but it
/// is also ten times the segmentation inference, and whether it buys
/// anything on top of what this pipeline already does is a measurement,
/// not a preference. Until that measurement says otherwise the cheap
/// geometry stands.
///
/// [`LiveDiarizer::with_step`] is how the harness varies it. Production
/// takes this constant.
pub const SEGMENT_STEP: usize = SEGMENT_WINDOW;

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

/// Grid the reconstruction aggregates onto.
///
/// The model's frame is ~17 ms and 589 of them do not divide evenly into a
/// window, let alone into a step that is a fraction of one. A running frame
/// index would drift a fraction of a frame per window and put that drift
/// into every timestamp it produced. A fixed grid cannot drift.
pub const GRID_MS: u64 = 10;

/// What production stamps on the vectors it stores, from the registry entry
/// for the model it actually loads.
///
/// The registry is the source. These two constants remain so the call sites
/// that read them do not all have to change, but they are now derived rather
/// than declared, and the value is byte-for-byte what was already stored.
pub const EMBEDDING_IDENTITY: VoiceprintId =
    crate::models::registry::DIARIZE_EMBEDDING.voiceprint();

/// The embedding model, as every vector it produces is labelled.
pub const EMBEDDING_MODEL: &str = EMBEDDING_IDENTITY.model;

/// Which front end fed it.
///
/// "1" was the pipeline that shipped with M3, whose filterbank was not the
/// one the model was trained on (see `fbank`'s module note); "2" is the
/// corrected one. A "1" vector and a "2" vector of the same audio agree at
/// cosine 0.36 on average, so they must never be compared: seeds are read
/// by version, and a "1" exemplar is rebuilt from its kept sample before
/// the next Diarization reads seeds at all (`cluster::adopt_rebuilt`).
pub const EMBEDDING_MODEL_VERSION: &str = EMBEDDING_IDENTITY.version;

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
    identity: VoiceprintId,
}

/// Where the model's filterbank lives — on the registry entry now, beside
/// the identity it belongs to, so the one place that names the model is the
/// one place that names its input. Re-exported here because this is where
/// every caller already looks for it.
pub use crate::models::registry::Frontend;

impl Embedder {
    pub fn load(path: &Path) -> Result<Self, DiarizeError> {
        Self::load_with(path, EMBEDDING_IDENTITY)
    }

    /// As [`load`](Self::load), for a harness holding a different model.
    ///
    /// The identity is a parameter because the stamp has to describe what
    /// was loaded. It used to be the constant, so every vector the A/B
    /// harness produced from ReDimNet2 was labelled WeSpeaker — harmless
    /// while the harness threw its store away, and a silent corruption the
    /// moment any other model shipped. The front end rides on the identity
    /// for the same reason: a model fed the other one's input returns a
    /// plausible vector of the wrong thing, and a caller that could name
    /// the model and the input separately could name them wrongly.
    pub fn load_with(path: &Path, identity: VoiceprintId) -> Result<Self, DiarizeError> {
        Ok(Self {
            session: open(path)?,
            mel: MelBank::new(),
            identity,
        })
    }

    /// What this embedder stamps on the vectors it produces.
    pub fn identity(&self) -> VoiceprintId {
        self.identity
    }

    pub fn frontend(&self) -> Frontend {
        self.identity.frontend
    }

    /// Embeds a stretch of raw audio, for a model that carries its own mel.
    /// `None` when there is too little of it to run the model on.
    ///
    /// The floor is the same [`MIN_EMBED_FRAMES`] the fbank path applies,
    /// spelled in samples: nine frames of `FRAME_LENGTH` at `FRAME_SHIFT`
    /// span `FRAME_LENGTH + 8 * FRAME_SHIFT` samples, not `9 * FRAME_SHIFT`.
    /// The two paths must refuse the same audio, or a stretch is a voice
    /// under one model and a cough under the other.
    pub fn embed_samples(&mut self, samples: &[f32]) -> Result<Option<Vec<f32>>, DiarizeError> {
        if samples.len() < FRAME_LENGTH + (MIN_EMBED_FRAMES - 1) * FRAME_SHIFT {
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

    /// Embeds a stretch of audio whole: one voice, on its own, in whatever
    /// form this model takes it.
    ///
    /// This is the entry every caller outside the live pass uses — the
    /// bounded re-seed, the exemplar rebuild — so the front end is chosen
    /// here and nowhere upstream. It used to compute fbank unconditionally,
    /// which was right for the one model that shipped and an `ort` input
    /// error for the other; `observe` never tripped it because it dispatches
    /// on the front end itself.
    pub fn embed(&mut self, samples: &[f32]) -> Result<Option<Vec<f32>>, DiarizeError> {
        match self.frontend() {
            Frontend::Waveform => self.embed_samples(samples),
            Frontend::Fbank => {
                let features = self.features(samples);
                let rows: Vec<&[f32]> = features.iter().map(Vec::as_slice).collect();
                self.embed_frames(&rows)
            }
        }
    }
}

/// The live Diarizer.
pub struct LiveDiarizer {
    segmentation: Session,
    embedder: Embedder,
    step: usize,
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
    /// Which segmentation window this came from, as an index into
    /// [`Observed::windows`], and which of that window's local speakers it
    /// is.
    ///
    /// Provenance, not input: nothing in clustering or assembly reads
    /// either. They are what lets two passes over the same audio with
    /// *different embedding models* be aligned observation to observation —
    /// which a measurement that holds the partition fixed and changes only
    /// the identity embedding needs, and which the runs cannot supply, since
    /// two local speakers of one window can share an active-frame pattern
    /// and then differ in nothing but their vectors.
    pub window: usize,
    pub local: u8,
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

/// Everything one pass of the models saw.
pub struct Observed {
    pub observations: Vec<Observation>,
    /// Every window the segmentation model ran, as `(channel, start, end)`
    /// on the capture clock.
    ///
    /// Which model produced these vectors, so what is stamped on them comes
    /// from the pass that made them rather than from a constant that may
    /// describe a different model entirely.
    pub embedding: VoiceprintId,
    /// **Kept even where a window produced no observation**, which is the
    /// whole reason this is carried separately rather than read off the
    /// observations. A window that looked and found nobody is a vote for
    /// silence, and a majority is not defined without the votes on both
    /// sides. Derived from the observations alone, a silent window would
    /// simply be absent and the instants around it would be held on a
    /// minority of the windows that actually saw them.
    pub windows: Vec<(AudioChannel, u64, u64)>,
}

/// The two channels, in the order everything in this module walks them.
///
/// They are separate recordings with their own windows, so a tally of who
/// was seen when cannot be shared between them.
const CHANNELS: [AudioChannel; 2] = [AudioChannel::Mic, AudioChannel::System];

/// Which half of a per-channel tally this channel owns.
fn slot(channel: AudioChannel) -> usize {
    usize::from(channel == AudioChannel::System)
}

/// The grid cells a span covers, clamped to what exists.
///
/// **Both edges round to the nearest cell rather than outward.** Flooring
/// the start and ceiling the end would widen every span by up to a cell at
/// each edge, and that error has a sign: a meeting's worth of turns would
/// each grow by up to 20 ms and the total would read as false alarm the
/// pipeline never produced. Rounding is unbiased, so quantising onto the
/// grid costs nothing on average — which is what lets the vote replace the
/// union without moving the score.
///
/// No run reaches this shorter than a cell: a local speaker needs
/// [`MIN_EMBED_FRAMES`] of about 150 ms to be embedded at all.
fn cells_of(start_ms: u64, end_ms: u64, cells: usize) -> std::ops::Range<usize> {
    let round = |ms: u64| (((ms + GRID_MS / 2) / GRID_MS) as usize).min(cells);
    let from = round(start_ms);
    let to = round(end_ms);
    from.min(to)..to
}

impl LiveDiarizer {
    /// Loads both models. Failure here is [`DiarizeError::Unavailable`] at
    /// the call site, never a lost Meeting.
    pub fn load(segmentation: &Path, embedding: &Path) -> Result<Self, DiarizeError> {
        Self::load_with(segmentation, embedding, EMBEDDING_IDENTITY)
    }

    /// How far the window advances, in milliseconds.
    ///
    /// For the harness, so the step can be measured without a rebuild;
    /// production keeps [`SEGMENT_STEP`]. Zero would not advance, so it is
    /// clamped to one sample rather than left to panic inside `step_by`.
    pub fn with_step(mut self, step_ms: u64) -> Self {
        self.step = ((step_ms * SAMPLE_RATE as u64 / 1000) as usize).max(1);
        self
    }

    /// As [`load`](Self::load), choosing which model the embedding is —
    /// what its vectors are labelled with and, with that, which front end
    /// it wants. The measurement harness is the caller; production takes
    /// the default.
    pub fn load_with(
        segmentation: &Path,
        embedding: &Path,
        identity: VoiceprintId,
    ) -> Result<Self, DiarizeError> {
        Ok(Self {
            step: SEGMENT_STEP,
            segmentation: open(segmentation)?,
            embedder: Embedder::load_with(embedding, identity)?,
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
    ) -> Result<Observed, DiarizeError> {
        if audio.sample_rate != SAMPLE_RATE {
            return Err(DiarizeError::Unavailable(format!(
                "diarization needs {SAMPLE_RATE} Hz audio, was given {}",
                audio.sample_rate
            )));
        }

        let total_ms = audio.duration_ms();
        let mut observations = Vec::new();
        let mut windows = Vec::new();

        for (channel, samples) in [
            (AudioChannel::Mic, audio.mic),
            (AudioChannel::System, audio.system),
        ] {
            for start in (0..samples.len()).step_by(self.step) {
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

                // Recorded before anything is known about who is in it: a
                // window that finds nobody still voted on every instant it
                // covered.
                windows.push((channel, chunk_start_ms, to_ms(masks.len())));
                let window = windows.len() - 1;

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
                            let picked = if alone_is_enough(masks, bit, samples_per_frame) {
                                runs_of(masks, |mask| mask == bit)
                            } else {
                                runs.clone()
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
                        window,
                        local: local as u8,
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
        Ok(Observed {
            observations,
            windows,
            embedding: self.embedder.identity(),
        })
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
    if alone_is_enough(masks, bit, samples_per_frame) {
        rows(&|mask| mask == bit)
    } else {
        rows(&|mask| mask & bit != 0)
    }
}

/// Whether one local speaker holds enough frames alone to be embedded from
/// them, rather than from all of its frames, overlap included.
///
/// **Both front ends ask this one function**, which is the whole point of it.
/// `chosen_rows` spends the answer as feature rows and the waveform path as
/// sample offsets, but they must be spending the same answer: a comparison
/// between two embeddings is only about the embeddings if everything upstream
/// picked the same audio. Counting is in feature rows either way — the
/// threshold was chosen against a 10 ms row, and segmentation frames are
/// nearer 17 ms, so counting those instead silently moves the bar.
pub fn alone_is_enough(masks: &[u8], bit: u8, samples_per_frame: f64) -> bool {
    (0..)
        .map(|row| ((row * FRAME_SHIFT + FRAME_LENGTH / 2) as f64 / samples_per_frame) as usize)
        .take_while(|frame| *frame < masks.len())
        .filter(|frame| masks[*frame] == bit)
        .count()
        > MIN_EMBED_FRAMES
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
///
/// **Turn coverage is settled by vote, not by union.** Every window that
/// covered an instant said whether a voice held it, and the instant belongs
/// to that voice when at least half of them agree. A boundary the model
/// wobbles on in one window is then stable in the answer.
///
/// Where windows do not overlap each instant has exactly one voter, so the
/// vote returns whatever that single window said and this *is* the union.
/// The rule only begins doing work once the step is smaller than
/// [`SEGMENT_WINDOW`] — which is the point of introducing it separately:
/// at today's step the output is unchanged, so a later difference is
/// attributable to the step and not to this rewrite.
///
/// The union cannot survive overlapping windows. Two windows disagreeing
/// about who holds an instant would both be believed, and the disagreement
/// reported as two people speaking at once.
pub fn assemble(observed: &Observed, canonical: &BTreeMap<Cluster, Cluster>) -> Diarization {
    let mut clean = Vec::new();
    let mut grouped: BTreeMap<Cluster, Vec<(Vec<f32>, i64, bool)>> = BTreeMap::new();

    // How many windows looked at each instant, per channel.
    let mut width = [0_usize; CHANNELS.len()];
    for (channel, _, end) in &observed.windows {
        let cells = end.div_ceil(GRID_MS) as usize;
        width[slot(*channel)] = width[slot(*channel)].max(cells);
    }
    let mut covers = [vec![0_u16; width[0]], vec![0_u16; width[1]]];
    for (channel, start, end) in &observed.windows {
        let row = &mut covers[slot(*channel)];
        let cells = row.len();
        for cell in cells_of(*start, *end, cells) {
            row[cell] += 1;
        }
    }

    // And how many of them put each voice there. **Windows, not
    // observations.** One window can hold two local tracks that clustering
    // later maps to the same voice, and where their runs overlap that one
    // window would otherwise vote twice for the same instant. Three windows
    // covering a cell, the other two silent, and a single window's two
    // tracks would carry it two-to-three — a majority assembled out of one
    // opinion. Each window gets one vote per voice per cell; `credited`
    // remembers which window last put this voice in this cell.
    let mut held: BTreeMap<(usize, Cluster), Vec<u16>> = BTreeMap::new();
    let mut credited: BTreeMap<(usize, Cluster), Vec<usize>> = BTreeMap::new();
    // By window, so one window's tracks are adjacent and the guard below
    // only ever has to remember the last one.
    let mut voting: Vec<&Observation> = observed.observations.iter().collect();
    voting.sort_by_key(|observation| observation.window);
    for observation in voting {
        let voice = canonical
            .get(&observation.cluster)
            .copied()
            .unwrap_or(observation.cluster);
        let cells = width[slot(observation.channel)];
        let key = (slot(observation.channel), voice);
        let counts = held.entry(key).or_insert_with(|| vec![0_u16; cells]);
        let credited = credited
            .entry(key)
            .or_insert_with(|| vec![usize::MAX; cells]);
        for (start, end) in &observation.runs {
            for cell in cells_of(*start, *end, cells) {
                if credited[cell] != observation.window {
                    credited[cell] = observation.window;
                    counts[cell] += 1;
                }
            }
        }
    }

    for observation in &observed.observations {
        let voice = canonical
            .get(&observation.cluster)
            .copied()
            .unwrap_or(observation.cluster);
        // Where to play a voice back from is a different question from
        // where it held the floor, and it keeps the union: a stretch this
        // voice had to itself is worth hearing whatever the neighbouring
        // windows made of the instant.
        clean.extend(
            observation
                .clean_runs
                .iter()
                .map(|(start, end)| Turn::new(observation.channel, *start, *end, voice.index())),
        );
        grouped.entry(voice).or_default().push((
            observation.vector.clone(),
            observation.voiced_ms() as i64,
            false,
        ));
    }

    let mut turns = Vec::new();
    for ((which, voice), counts) in &held {
        let row = &covers[*which];
        let channel = &CHANNELS[*which];
        let mut open: Option<usize> = None;
        for cell in 0..counts.len() {
            // A cell nobody looked at is outside the recording; a cell ten
            // windows looked at needs five of them.
            let active = row[cell] > 0 && u32::from(counts[cell]) * 2 >= u32::from(row[cell]);
            match (active, open) {
                (true, None) => open = Some(cell),
                (false, Some(from)) => {
                    turns.push(Turn::new(
                        *channel,
                        from as u64 * GRID_MS,
                        cell as u64 * GRID_MS,
                        voice.index(),
                    ));
                    open = None;
                }
                _ => {}
            }
        }
        if let Some(from) = open {
            turns.push(Turn::new(
                *channel,
                from as u64 * GRID_MS,
                counts.len() as u64 * GRID_MS,
                voice.index(),
            ));
        }
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
                // The pass that made these vectors, not the constant — the
                // same fix ticket 04 made to `provisional_of`, and the same
                // reason: this is the stamp that reaches the record, so a
                // constant here labels a measured model's vectors with
                // production's name.
                observed.embedding.model,
                observed.embedding.version,
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

/// Group observations into voices and lay them out as turns: everything
/// [`LiveDiarizer::diarize`] does once the models have spoken.
///
/// Split out of `diarize` so one meeting's observations can be clustered more
/// than once at more than one merge threshold, paying for the inference only
/// once. A harness that re-implemented this half instead would be free to
/// drift from what production does, which is the one thing a measurement of
/// production must not do. **Production reaches it only through `diarize`**,
/// which passes [`super::cluster::MERGE_THRESHOLD`].
pub fn cluster_observed(observed: &Observed, threshold: f32) -> Diarization {
    let provisional = provisional_of(observed);
    let canonical = super::cluster::agglomerate_with(&provisional, threshold);
    assemble(observed, &canonical)
}

/// [`cluster_observed`] with segmentation's own cannot-link pairs enforced.
///
/// Harness-only, and the reason the constraint can be measured without
/// shipping it: production still calls `agglomerate`, which has no
/// constraints and never did.
pub fn cluster_observed_constrained(observed: &Observed, threshold: f32) -> Diarization {
    let provisional = provisional_of(observed);
    let forbidden = cannot_link_of(observed);
    let canonical = super::cluster::agglomerate_constrained(&provisional, threshold, &forbidden);
    assemble(observed, &canonical)
}

/// Every observation as its own cluster, which is where agglomeration starts.
///
/// Public so a measurement can reach the partition itself rather than only
/// the turns it produces — the cannot-link question ticket 03 raised is about
/// which observations ended up together, which `assemble` has already thrown
/// away by the time it returns.
pub fn provisional_of(observed: &Observed) -> BTreeMap<Cluster, Embedding> {
    observed
        .observations
        .iter()
        .map(|observation| {
            (
                observation.cluster,
                Embedding::new(
                    observation.vector.clone(),
                    observed.embedding.model,
                    observed.embedding.version,
                    observation.voiced_ms(),
                ),
            )
        })
        .collect()
}

/// Pairs of provisional clusters that segmentation says are different people.
///
/// Two local speakers of one window are two *because* the segmentation model
/// separated them there, so merging them later contradicts the evidence the
/// pipeline already has. Nothing in [`super::cluster::agglomerate`] knows
/// that, and no fixture can catch it: fixture vectors are orthogonal and
/// never come close enough to merge.
///
/// **Provenance only.** The pairs come from which window and channel an
/// observation was produced in, never from what a reference transcript says
/// about it. A constraint derived from the answer key would be an oracle:
/// it would improve the measurement and could not ship, and it would hide
/// exactly the cost worth knowing, which is what happens when segmentation
/// splits one person into two local tracks and this refuses to put them
/// back together.
///
/// Same channel only, which follows from grouping by window: a window
/// belongs to one channel. The two channels are separate recordings with
/// their own windows, and one person can be on both, so simultaneity across
/// them says nothing.
///
/// Two local tracks are a *hypothesis* that two people are talking, not a
/// proof of it. Segmentation can split one person in two, and this then
/// refuses to put them back together; that cost belongs in whatever the
/// constraint is measured to be worth.
///
/// Harness-only, like [`provisional_of`]. Returns both directions of every
/// pair, because [`super::cluster::agglomerate_constrained`] checks one.
pub fn cannot_link_of(observed: &Observed) -> BTreeMap<Cluster, BTreeSet<Cluster>> {
    // Grouped by the window each observation **records**, not by the window
    // its first run happens to land in. Under a sliding step an instant sits
    // in several windows, so geometry cannot name the one that produced an
    // observation — it would pick whichever matched first. The observation
    // has said all along: `window` and `local` are written by `observe` at
    // the moment the vectors are cut. Overlap in the audio does not make the
    // source ambiguous, so a sliding step needs no special case here.
    let mut together: BTreeMap<usize, Vec<(u8, Cluster)>> = BTreeMap::new();
    for observation in &observed.observations {
        let Some(&(channel, _, _)) = observed.windows.get(observation.window) else {
            panic!(
                "observation claims window {} of {}; its provenance cannot be \
                 trusted to say who it may not be",
                observation.window,
                observed.windows.len()
            );
        };
        assert_eq!(
            channel, observation.channel,
            "observation on {:?} claims window {}, which ran on {channel:?}. \
             The two channels are separate recordings with their own windows, \
             so a pair drawn across them would constrain nothing real.",
            observation.channel, observation.window
        );
        together
            .entry(observation.window)
            .or_default()
            .push((observation.local, observation.cluster));
    }

    let mut forbidden: BTreeMap<Cluster, BTreeSet<Cluster>> = BTreeMap::new();
    for tracks in together.values() {
        for (index, (left_local, left)) in tracks.iter().enumerate() {
            for (right_local, right) in &tracks[index + 1..] {
                // One local track is one hypothesised person. Two rows of
                // the same track are not two people, and nothing about the
                // segmentation says they may not be the same voice.
                if left_local == right_local {
                    continue;
                }
                forbidden.entry(*left).or_default().insert(*right);
                forbidden.entry(*right).or_default().insert(*left);
            }
        }
    }
    forbidden
}

impl Diarizer for LiveDiarizer {
    fn diarize(
        &mut self,
        audio: MeetingAudio<'_>,
        progress: &mut dyn FnMut(Progress),
        cancel: &Cancel,
    ) -> Result<Diarization, DiarizeError> {
        let observed = self.observe(audio, progress, cancel)?;
        let result = cluster_observed(&observed, super::cluster::MERGE_THRESHOLD);

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

    /// The only local track of its window.
    ///
    /// Provenance is named rather than guessed at. It used to be stamped
    /// `window: cluster`, which made every fixture's window index a
    /// coincidence of its cluster number: the three repeated cluster-0
    /// observations below all claimed window 0, and the tiled cannot-link
    /// fixture put its two same-window tracks in different windows. Both
    /// now say which window they came from, because the code under test
    /// reads that field.
    fn observation(
        window: usize,
        channel: AudioChannel,
        cluster: u32,
        runs: &[(u64, u64)],
        clean: &[(u64, u64)],
    ) -> Observation {
        track(window, 0, channel, cluster, runs, clean)
    }

    /// One named local track of a window, for the fixtures where two tracks
    /// of the *same* window are the point.
    fn track(
        window: usize,
        local: u8,
        channel: AudioChannel,
        cluster: u32,
        runs: &[(u64, u64)],
        clean: &[(u64, u64)],
    ) -> Observation {
        Observation {
            channel,
            cluster: Cluster(cluster),
            window,
            local,
            vector: vec![1.0, 0.0],
            runs: runs.to_vec(),
            clean_runs: clean.to_vec(),
        }
    }

    /// Overlapping windows are ordinary now, because provenance is recorded.
    ///
    /// This used to refuse them: with windows that tile, "the window an
    /// observation came from" could be answered by asking which window
    /// contains its first run, and under a sliding step that question has
    /// several answers, so the guard refused rather than pick one. But the
    /// observation records the window `observe` cut it from, so there was
    /// never a need to infer it — and the refusal is what kept the
    /// diagnostic from being computable at the very steps it was wanted for.
    ///
    /// The case that makes the difference: this observation is from the
    /// *later* window, and its first run also falls inside the earlier one.
    /// Asking geometry returns window 0, the earliest match, which is the
    /// wrong source and would pair it with the wrong tracks.
    #[test]
    fn a_run_inside_an_earlier_window_is_still_the_window_it_came_from() {
        assert_eq!(
            SEGMENT_STEP, SEGMENT_WINDOW,
            "the defaults are unchanged; only these windows overlap"
        );
        let overlapped = Observed {
            embedding: EMBEDDING_IDENTITY,
            observations: vec![
                // Both from window 1, though 6000 ms sits in window 0 too.
                track(1, 0, AudioChannel::Mic, 0, &[(6_000, 7_000)], &[]),
                track(1, 1, AudioChannel::Mic, 1, &[(6_000, 7_000)], &[]),
                // And one genuinely from window 0, which geometry would
                // have lumped in with them.
                track(0, 0, AudioChannel::Mic, 2, &[(1_000, 2_000)], &[]),
            ],
            windows: vec![
                (AudioChannel::Mic, 0, 10_000),
                (AudioChannel::Mic, 5_000, 15_000),
            ],
        };

        let forbidden = cannot_link_of(&overlapped);
        assert_eq!(forbidden[&Cluster(0)], BTreeSet::from([Cluster(1)]));
        assert_eq!(forbidden[&Cluster(1)], BTreeSet::from([Cluster(0)]));
        assert!(
            !forbidden.contains_key(&Cluster(2)),
            "the only track of window 0, so it is forbidden nothing — and \
             inferring its window from geometry would have forbidden it both"
        );
    }

    /// Two rows of one local track are one track.
    ///
    /// Segmentation separating two people is what a constraint is entitled
    /// to read; the same track appearing twice says nothing, and forbidding
    /// it to itself would refuse a merge nothing ruled out.
    #[test]
    fn one_local_track_is_not_forbidden_to_itself() {
        let doubled = Observed {
            embedding: EMBEDDING_IDENTITY,
            observations: vec![
                track(0, 0, AudioChannel::Mic, 0, &[(1_000, 2_000)], &[]),
                track(0, 0, AudioChannel::Mic, 1, &[(3_000, 4_000)], &[]),
            ],
            windows: vec![(AudioChannel::Mic, 0, 10_000)],
        };
        assert!(cannot_link_of(&doubled).is_empty());
    }

    /// Silently skipping it would quietly build fewer constraints than the
    /// run believes it has, which is the failure this whole guard exists to
    /// make impossible.
    #[test]
    #[should_panic(expected = "claims window 4")]
    fn an_observation_naming_a_window_that_does_not_exist_is_refused() {
        let stray = Observed {
            embedding: EMBEDDING_IDENTITY,
            observations: vec![observation(4, AudioChannel::Mic, 0, &[(1_000, 2_000)], &[])],
            windows: vec![(AudioChannel::Mic, 0, 10_000)],
        };
        let _ = cannot_link_of(&stray);
    }

    /// The stamp says which model ran, not which model usually runs.
    ///
    /// It used to be the constant. The whole ReDimNet2 A/B therefore
    /// labelled its vectors WeSpeaker — invisible while the harness threw
    /// its store away each run, and a silent corruption the first time a
    /// second model shipped, because `seeds` would then have handed
    /// WeSpeaker Voiceprints to a ReDimNet2 resolve.
    #[test]
    fn the_stamp_names_the_model_that_actually_ran() {
        let other = VoiceprintId {
            model: "wespeaker-voxceleb-resnet34-LM",
            version: "2",
            frontend: Frontend::Fbank,
        };
        let observed = Observed {
            embedding: other,
            observations: vec![observation(0, AudioChannel::Mic, 0, &[(0, 1_000)], &[])],
            windows: vec![(AudioChannel::Mic, 0, 10_000)],
        };
        let stamped = provisional_of(&observed);
        let one = stamped.values().next().expect("one observation");
        assert_eq!(
            (one.model.as_str(), one.model_version.as_str()),
            ("wespeaker-voxceleb-resnet34-LM", "2")
        );
        assert_ne!(one.model, EMBEDDING_MODEL);
    }

    /// The assembled vector carries the same stamp the provisional one did.
    ///
    /// `assemble` is the stamp that reaches the record — `persist` files
    /// these — so a constant here would label a measured model's vectors
    /// with production's name however truthfully `provisional_of` had
    /// stamped them a moment earlier.
    #[test]
    fn the_assembled_stamp_names_the_model_that_actually_ran() {
        let observed = Observed {
            embedding: VoiceprintId {
                model: "wespeaker-voxceleb-resnet34-LM",
                version: "2",
                frontend: Frontend::Fbank,
            },
            observations: vec![observation(
                0,
                AudioChannel::Mic,
                0,
                &[(0, 2_000)],
                &[(0, 2_000)],
            )],
            windows: vec![(AudioChannel::Mic, 0, 10_000)],
        };
        let canonical = [(Cluster(0), Cluster(0))].into_iter().collect();
        let assembled = assemble(&observed, &canonical);
        let embedding = assembled.embeddings.values().next().expect("one voice");
        assert_eq!(
            (embedding.model.as_str(), embedding.model_version.as_str()),
            ("wespeaker-voxceleb-resnet34-LM", "2")
        );
        assert_ne!(embedding.model, EMBEDDING_MODEL);
    }

    /// Production's own stamp still comes from the registry, unchanged.
    #[test]
    fn production_stamps_what_the_registry_says_it_stores() {
        assert_eq!(
            (EMBEDDING_MODEL, EMBEDDING_MODEL_VERSION),
            ("redimnet2-b3", "1")
        );
        assert_eq!(
            EMBEDDING_IDENTITY,
            crate::models::registry::DIARIZE_EMBEDDING.voiceprint()
        );
    }

    #[test]
    fn two_local_speakers_of_one_window_may_never_be_one_voice() {
        let tiled = Observed {
            embedding: EMBEDDING_IDENTITY,
            observations: vec![
                track(0, 0, AudioChannel::Mic, 0, &[(1_000, 2_000)], &[]),
                track(0, 1, AudioChannel::Mic, 1, &[(1_500, 2_500)], &[]),
                observation(1, AudioChannel::Mic, 2, &[(11_000, 12_000)], &[]),
                // Same instant, other channel: separate recordings with
                // their own windows, and one person can be on both, so
                // simultaneity across them says nothing.
                observation(2, AudioChannel::System, 3, &[(1_200, 2_200)], &[]),
            ],
            windows: vec![
                (AudioChannel::Mic, 0, 10_000),
                (AudioChannel::Mic, 10_000, 20_000),
                (AudioChannel::System, 0, 10_000),
            ],
        };

        let forbidden = cannot_link_of(&tiled);
        assert_eq!(forbidden[&Cluster(0)], BTreeSet::from([Cluster(1)]));
        assert_eq!(forbidden[&Cluster(1)], BTreeSet::from([Cluster(0)]));
        assert!(
            !forbidden.contains_key(&Cluster(2)),
            "alone in its window, so it is forbidden nothing"
        );
        assert!(
            !forbidden.contains_key(&Cluster(3)),
            "the other channel is not evidence about this one"
        );
    }

    /// A run whose windows did not overlap: one window per channel,
    /// spanning everything that channel saw.
    ///
    /// Every instant then has exactly one voter, which is the geometry
    /// production uses today and the one every test below was written
    /// against. `assemble`'s vote returns the union under it, so these
    /// tests are also the evidence that introducing the vote changed
    /// nothing at the current step.
    fn observed(observations: Vec<Observation>) -> Observed {
        let windows = CHANNELS
            .into_iter()
            .filter_map(|channel| {
                let end = observations
                    .iter()
                    .filter(|observation| observation.channel == channel)
                    .flat_map(|observation| {
                        observation.runs.iter().chain(observation.clean_runs.iter())
                    })
                    .map(|(_, end)| *end)
                    .max()?;
                Some((channel, 0, end))
            })
            .collect();
        Observed {
            embedding: EMBEDDING_IDENTITY,
            observations,
            windows,
        }
    }

    #[test]
    fn a_minority_of_windows_does_not_carry_an_instant() {
        // The property a union cannot express, and the whole reason a
        // sliding step needs a vote. Three windows covered this second;
        // all three put voice 0 there and only one put voice 1 there.
        // Under a union that single dissenting window would have been
        // believed and the second reported as two people talking at once.
        let observed = Observed {
            embedding: EMBEDDING_IDENTITY,
            observations: vec![
                observation(0, AudioChannel::Mic, 0, &[(0, 3_000)], &[(0, 3_000)]),
                observation(1, AudioChannel::Mic, 0, &[(0, 3_000)], &[(0, 3_000)]),
                observation(2, AudioChannel::Mic, 0, &[(0, 3_000)], &[(0, 3_000)]),
                track(0, 1, AudioChannel::Mic, 1, &[(1_000, 2_000)], &[]),
            ],
            windows: vec![(AudioChannel::Mic, 0, 3_000); 3],
        };
        let result = assemble(&observed, &BTreeMap::new());
        assert_eq!(
            result.turns.len(),
            1,
            "only the voice a majority heard holds the floor: {:?}",
            result.turns
        );
        assert_eq!(result.turns[0].cluster, Cluster(0));
    }

    /// One window cannot outvote the two that disagreed with it.
    ///
    /// Segmentation splits a voice into two local tracks inside a single
    /// window more or less constantly, and clustering then puts them back
    /// together — at which point a vote that counted *observations* heard
    /// that window say the same thing twice. Three windows cover this
    /// second, two of them found nobody, and the third found this voice on
    /// two overlapping tracks: two votes against three voters is a majority
    /// assembled out of one opinion. The window votes once.
    #[test]
    fn two_local_tracks_of_one_window_are_one_vote() {
        let observed = Observed {
            embedding: EMBEDDING_IDENTITY,
            observations: vec![
                track(0, 0, AudioChannel::Mic, 0, &[(1_000, 2_000)], &[]),
                track(0, 1, AudioChannel::Mic, 1, &[(1_000, 2_000)], &[]),
            ],
            windows: vec![(AudioChannel::Mic, 0, 3_000); 3],
        };
        // Clustering decided the two tracks are the same person.
        let canonical = [(Cluster(0), Cluster(0)), (Cluster(1), Cluster(0))]
            .into_iter()
            .collect();
        let result = assemble(&observed, &canonical);
        assert!(
            result.turns.is_empty(),
            "one window of three is not a majority, however many tracks it \
             split the voice across: {:?}",
            result.turns
        );
    }

    /// And a window is still one vote when its tracks are not adjacent.
    ///
    /// The guard remembers only the window that last credited a cell, which
    /// is enough because the votes are sorted by window first. Take the sort
    /// away and an order like window 0, window 1, window 0 slips a second
    /// vote past it — so this fixture is deliberately in that order, and the
    /// two tracks of window 0 straddle a track of window 1. Two windows of
    /// five, not three.
    #[test]
    fn a_window_whose_tracks_are_not_adjacent_still_votes_once() {
        let observed = Observed {
            embedding: EMBEDDING_IDENTITY,
            observations: vec![
                track(0, 0, AudioChannel::Mic, 0, &[(1_000, 2_000)], &[]),
                track(1, 0, AudioChannel::Mic, 1, &[(1_000, 2_000)], &[]),
                track(0, 1, AudioChannel::Mic, 2, &[(1_000, 2_000)], &[]),
            ],
            windows: vec![(AudioChannel::Mic, 0, 3_000); 5],
        };
        let canonical = [
            (Cluster(0), Cluster(0)),
            (Cluster(1), Cluster(0)),
            (Cluster(2), Cluster(0)),
        ]
        .into_iter()
        .collect();
        assert!(
            assemble(&observed, &canonical).turns.is_empty(),
            "two of five windows is not a majority"
        );
    }

    /// The control that keeps the fix from being "count nothing".
    ///
    /// Same shape, same clustering, same three voters — but the two tracks
    /// come from *different* windows, so two of three really did put this
    /// voice here and the instant is carried.
    #[test]
    fn two_windows_that_agree_still_carry_the_instant() {
        let observed = Observed {
            embedding: EMBEDDING_IDENTITY,
            observations: vec![
                track(0, 0, AudioChannel::Mic, 0, &[(1_000, 2_000)], &[]),
                track(1, 0, AudioChannel::Mic, 1, &[(1_000, 2_000)], &[]),
            ],
            windows: vec![(AudioChannel::Mic, 0, 3_000); 3],
        };
        let canonical = [(Cluster(0), Cluster(0)), (Cluster(1), Cluster(0))]
            .into_iter()
            .collect();
        let result = assemble(&observed, &canonical);
        assert_eq!(result.turns.len(), 1, "{:?}", result.turns);
        assert_eq!(result.turns[0].cluster, Cluster(0));
        assert_eq!(result.turns[0].duration_ms(), 1_000);
    }

    #[test]
    fn half_the_windows_are_enough() {
        // Two of four is a majority for this purpose: a boundary the model
        // puts inside a window as often as outside it should land, not
        // vanish. The threshold is `>=`, and this is the case that says so.
        let observed = Observed {
            embedding: EMBEDDING_IDENTITY,
            observations: vec![
                observation(0, AudioChannel::Mic, 0, &[(0, 1_000)], &[]),
                observation(1, AudioChannel::Mic, 0, &[(0, 1_000)], &[]),
            ],
            windows: vec![(AudioChannel::Mic, 0, 1_000); 4],
        };
        let result = assemble(&observed, &BTreeMap::new());
        assert_eq!(result.turns.len(), 1, "{:?}", result.turns);
        assert_eq!(result.turns[0].duration_ms(), 1_000);
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
    fn both_front_ends_choose_the_same_frames() {
        // The A/B between two embeddings is only about the embeddings if
        // everything upstream picks the same audio. `chosen_rows` spends
        // the answer as feature rows and the waveform path as sample
        // offsets; this pins that they are spending the same answer.
        //
        // Two short alone-runs is the case that used to diverge: the
        // waveform path branched on the *number of runs*, so it took the
        // clean frames where the fbank path, counting rows against the
        // minimum, took everything. Different audio, silently.
        let mut masks = vec![0_u8; 589];
        masks[100..200].fill(0b11);
        masks[200..202].fill(0b10);
        masks[300..302].fill(0b10); // two alone-runs, four frames between them
        assert!(
            !alone_is_enough(&masks, 0b10, SAMPLES_PER_FRAME),
            "four frames is under the minimum however many runs they arrive in"
        );

        for (masks, bit, alone) in [
            (masks.clone(), 0b10_u8, false),
            (
                {
                    let mut m = vec![0b01_u8; 589];
                    m[295..].fill(0b11);
                    m
                },
                0b01_u8,
                true,
            ),
        ] {
            let enough = alone_is_enough(&masks, bit, SAMPLES_PER_FRAME);
            assert_eq!(enough, alone);
            // The fbank path's rows and the waveform path's runs must come
            // from the same predicate, so every row's centre falls inside
            // some run the waveform path would have taken.
            let runs = if enough {
                runs_of(&masks, |mask| mask == bit)
            } else {
                runs_of(&masks, |mask| mask & bit != 0)
            };
            for row in chosen_rows(&masks, bit, SAMPLES_PER_FRAME) {
                let centre =
                    ((row * FRAME_SHIFT + FRAME_LENGTH / 2) as f64 / SAMPLES_PER_FRAME) as usize;
                assert!(
                    runs.iter()
                        .any(|(from, to)| centre >= *from && centre < *to),
                    "row {row} (frame {centre}) is outside every run the waveform path takes"
                );
            }
        }
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
            track(0, 0, AudioChannel::Mic, 0, &[(0, 5_000)], &[(0, 3_000)]),
            track(
                0,
                1,
                AudioChannel::Mic,
                1,
                &[(3_000, 8_000)],
                &[(5_000, 8_000)],
            ),
        ];
        let result = assemble(&observed(observations), &BTreeMap::new());
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
            0,
            AudioChannel::System,
            0,
            &[(1_000, 1_200)],
            &[(1_000, 1_200)],
        )];
        let result = assemble(&observed(observations), &BTreeMap::new());
        assert_eq!(result.turns.len(), 1);
        assert_eq!(result.turns[0].duration_ms(), 200);
        assert!(
            result.embeddings[&Cluster(0)].sample.is_none(),
            "too short to play"
        );
    }

    #[test]
    fn two_local_speakers_of_one_voice_are_one_turn() {
        // The same person on two local tracks of one window, clustered
        // together. Their runs do not overlap, so the vote counts the
        // window once either way and `merge_adjacent` is what joins them.
        let observations = vec![
            track(
                0,
                0,
                AudioChannel::Mic,
                0,
                &[(7_000, 10_000)],
                &[(7_000, 10_000)],
            ),
            track(
                0,
                1,
                AudioChannel::Mic,
                1,
                &[(10_000, 12_000)],
                &[(10_000, 12_000)],
            ),
        ];
        let canonical = [(Cluster(0), Cluster(0)), (Cluster(1), Cluster(0))]
            .into_iter()
            .collect();
        let result = assemble(&observed(observations), &canonical);
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
            0,
            AudioChannel::Mic,
            0,
            &[(0, 10_000)],
            &[(0, 1_000), (4_000, 7_000)],
        )];
        let result = assemble(&observed(observations), &BTreeMap::new());
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
        assert_eq!(vector.len(), 192, "the embedding model's stated width");
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
