//! The local ONNX pipeline: segmentation, embedding, clustering.
//!
//! Two models, both run through `ort`, both entirely on this machine —
//! story 33 forbids a cloud form in any shape, so there is no client, no
//! endpoint, and no fallback path to review.
//!
//! **Turns come from segmentation.** The window slides by
//! [`SEGMENT_STEP`] — one tenth of itself, which is what pyannote's
//! published 18.8% on AMI uses — and every window says which of its
//! [`LOCAL_SPEAKERS`] held which frames. Each local speaker is embedded from
//! the frames it holds with everybody else's removed, all those vectors are
//! clustered together, and the timeline is then rebuilt per voice:
//!
//!   1. `LiveDiarizer::segment` runs the window across the channel and
//!      keeps per-frame *per-speaker* activation, not a speaker count.
//!   2. Each (window, local speaker) with at least [`MIN_EMBED_MS`] of
//!      speech is embedded from its own frames alone.
//!   3. [`super::cluster::agglomerate`] groups those vectors into voices.
//!   4. [`reconstruct`] rebuilds each voice's turns by majority vote across
//!      the windows that saw each instant.
//!
//! **A local speaker index is not an identity.** Local speaker 1 in one
//! window and local speaker 1 in the next are unrelated; the slot is an
//! index into that window's opinion. Identity is recovered in step 3 and
//! nowhere else, which is why nothing here tries to stitch slots across the
//! overlap.
//!
//! The version this replaced reduced segmentation to a per-frame speaker
//! *count*, treated a frame as speech only when exactly one voice held it,
//! and cut turns out of material long enough to embed. On AMI that left 27%
//! of speaker-time overlapped and unattributable and another 6% too short
//! to embed, for a 32.6% floor that perfect clustering could not get under.

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
use super::fbank::MelBank;
use super::fbank::SAMPLE_RATE;

/// Window the segmentation model was trained on: 10 s at 16 kHz.
pub const SEGMENT_WINDOW: usize = 10 * SAMPLE_RATE as usize;

/// How far the window moves between runs.
///
/// A tenth of the window, so every instant is seen by ten of them and a
/// boundary the model places differently in one window is outvoted. This is
/// the step pyannote's published number uses, and it is also the pipeline's
/// cost: one embedding per active local speaker per window means ten times
/// the windows buys roughly ten times the embedding work.
///
/// ponytail: a 2 s step is half the compute; ticket 03 asks for it to be
/// measured through the DER harness and kept only if DER holds within a
/// point.
pub const SEGMENT_STEP: usize = SEGMENT_WINDOW / 10;

/// Powerset classes for three speakers: none, three singles, three pairs.
pub const POWERSET_CLASSES: usize = 7;

/// How many speakers the powerset head can tell apart inside one window.
pub const LOCAL_SPEAKERS: usize = 3;

/// Which speakers each powerset class means are active.
///
/// Order is the model's, not ours. Getting this wrong produces a pipeline
/// that runs perfectly and mislabels every overlap — the exact class of
/// silent error the fbank module is also written to avoid.
const POWERSET: [&[usize]; POWERSET_CLASSES] = [&[], &[0], &[1], &[2], &[0, 1], &[0, 2], &[1, 2]];

/// Resolution the timeline is rebuilt on.
///
/// The model's own frame is about 17 ms and does not divide evenly into the
/// window step, so windows are aggregated onto a fixed grid. A global frame
/// index would drift by a fraction of a frame every step and put the drift
/// into every timestamp.
pub const GRID_MS: u64 = 10;

/// Shortest stretch worth embedding *for clustering*.
///
/// Not [`MIN_SPAN_MS`], and the difference is the point. This is the floor
/// below which a vector is too short to say anything about which voice a
/// window's local speaker is; a half-second "mm-hm" is above it, gets a
/// vector, and therefore gets a speaker. What a *Voiceprint* may be built
/// from is a separate and much stricter question, answered by
/// [`MIN_SPAN_MS`] and [`super::cluster::MIN_SPEAKER_MS`].
pub const MIN_EMBED_MS: u64 = 250;

/// Shortest span worth holding a voice up by.
///
/// The catalog's minimum voiced duration. Below this a Voiceprint is built
/// from too little evidence to be worth more than no Voiceprint at all.
/// This governs [`embeddable`] only: it never decides whether speech gets a
/// turn.
pub const MIN_SPAN_MS: u64 = 1_500;

/// Longest span played back for a voice — the catalog's middle-10s clip.
pub const MAX_SPAN_MS: u64 = 10_000;

/// Gaps shorter than this do not end a turn (catalog: 400 ms merge gap).
pub const MERGE_GAP_MS: u64 = 400;

// Turn coverage and embeddable span are different questions, and the
// pipeline fails quietly if the two floors ever converge: a clustering floor
// at or above the Voiceprint minimum returns every short turn to having no
// speaker at all, which is the defect this ticket was written to remove.
// Checked at compile time because there is no build in which it is allowed.
const _: () = assert!(MIN_EMBED_MS < MIN_SPAN_MS);

/// What one sliding window saw.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LocalWindow {
    pub start_ms: u64,
    pub end_ms: u64,
    /// `held[local]` — the ranges that local speaker holds, on the capture
    /// clock. The index means nothing outside this window.
    pub held: Vec<Vec<(u64, u64)>>,
}

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
            // Read from the registry rather than written here. A literal in
            // this constructor is an account of what produced a Voiceprint
            // that nobody updates when the model behind it changes, and the
            // guard in `cluster::seeds` is only as true as this stamp
            // (ADR-0037).
            model_name: crate::models::registry::DIARIZE_EMBEDDING.key.to_string(),
            model_version: crate::models::registry::DIARIZE_EMBEDDING
                .version
                .to_string(),
        })
    }

    /// Slides the segmentation window across one channel.
    fn segment(&mut self, samples: &[f32]) -> Result<Vec<LocalWindow>, DiarizeError> {
        if samples.is_empty() {
            return Ok(Vec::new());
        }

        let mut windows = Vec::new();
        let mut start = 0_usize;
        loop {
            let end = (start + SEGMENT_WINDOW).min(samples.len());
            let mut buffer = vec![0.0_f32; SEGMENT_WINDOW];
            buffer[..end - start].copy_from_slice(&samples[start..end]);

            let input = Value::from_array(([1_usize, 1, SEGMENT_WINDOW], buffer))
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
            // Derived from the model's own output rather than assumed: a
            // hard-coded frame duration is how every timestamp in a pipeline
            // ends up scaled by a constant nobody notices.
            let frame_ms = (SEGMENT_WINDOW as f64 / SAMPLE_RATE as f64) * 1000.0 / frames as f64;

            // The tail of the last window is zero padding this code
            // invented; reading speech out of it would run the meeting on
            // past its own end.
            let covered = ((end - start) as f64 / SEGMENT_WINDOW as f64 * frames as f64) as usize;
            let argmax: Vec<usize> = (0..frames.min(covered))
                .map(|frame| {
                    logits[frame * classes..(frame + 1) * classes]
                        .iter()
                        .enumerate()
                        .max_by(|a, b| a.1.total_cmp(b.1))
                        .map(|(index, _)| index)
                        .unwrap_or(0)
                })
                .collect();

            let start_ms = start as u64 * 1000 / SAMPLE_RATE as u64;
            windows.push(LocalWindow {
                start_ms,
                end_ms: start_ms + (argmax.len() as f64 * frame_ms) as u64,
                held: window_hold(&argmax, start_ms, frame_ms),
            });

            if end == samples.len() {
                break;
            }
            start += SEGMENT_STEP;
        }
        Ok(windows)
    }

    /// Embeds one stretch of audio.
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

/// Expands the powerset argmax into one activation row per local speaker.
///
/// The step the old pipeline skipped: it counted how many speakers a class
/// meant and threw away which, which is the whole of the identity the
/// segmentation model offers.
pub fn activations(classes: &[usize]) -> Vec<Vec<bool>> {
    let mut active = vec![vec![false; classes.len()]; LOCAL_SPEAKERS];
    for (frame, class) in classes.iter().enumerate() {
        for local in POWERSET.get(*class).copied().unwrap_or(&[]) {
            if let Some(row) = active.get_mut(*local) {
                row[frame] = true;
            }
        }
    }
    active
}

/// Turns one window's per-frame classes into per-local-speaker ranges.
pub fn window_hold(classes: &[usize], start_ms: u64, frame_ms: f64) -> Vec<Vec<(u64, u64)>> {
    let at = |frame: usize| start_ms + (frame as f64 * frame_ms) as u64;
    activations(classes)
        .into_iter()
        .map(|row| {
            let mut ranges: Vec<(u64, u64)> = Vec::new();
            let mut open: Option<usize> = None;
            for (frame, on) in row.iter().enumerate() {
                match (*on, open) {
                    (true, None) => open = Some(frame),
                    (false, Some(from)) => {
                        ranges.push((at(from), at(frame)));
                        open = None;
                    }
                    _ => {}
                }
            }
            if let Some(from) = open {
                ranges.push((at(from), at(row.len())));
            }
            ranges
        })
        .collect()
}

fn cells_of(start_ms: u64, end_ms: u64, cells: usize) -> std::ops::Range<usize> {
    let from = ((start_ms / GRID_MS) as usize).min(cells);
    let to = ((end_ms.div_ceil(GRID_MS)) as usize).min(cells);
    from.min(to)..to
}

/// How much speech a set of ranges holds.
pub fn held_ms(ranges: &[(u64, u64)]) -> u64 {
    ranges
        .iter()
        .map(|(start, end)| end.saturating_sub(*start))
        .sum()
}

/// Rebuilds each voice's turns from every window that saw it.
///
/// `voices` maps `(window index, local speaker)` to the voice clustering put
/// it in; a pair with no entry had too little speech to embed and
/// contributes nothing. An instant belongs to a voice when at least half the
/// windows covering it said so, which is what makes a boundary the model
/// wobbles on in one window stable in the answer.
///
/// Two voices may hold the same instant, and nothing is dropped for being
/// brief: both of those are the point.
pub fn reconstruct(
    windows: &[LocalWindow],
    voices: &BTreeMap<(usize, usize), Cluster>,
    channel: AudioChannel,
) -> Vec<Turn> {
    let total_ms = windows
        .iter()
        .map(|window| window.end_ms)
        .max()
        .unwrap_or(0);
    if total_ms == 0 {
        return Vec::new();
    }
    let cells = (total_ms.div_ceil(GRID_MS)) as usize;

    // How many windows saw each instant at all. A cell nobody covered is
    // outside the recording; a cell ten windows covered needs five to agree.
    let mut covers = vec![0_u16; cells];
    for window in windows {
        for cell in cells_of(window.start_ms, window.end_ms, cells) {
            covers[cell] += 1;
        }
    }

    let mut held: BTreeMap<Cluster, Vec<u16>> = BTreeMap::new();
    for (index, window) in windows.iter().enumerate() {
        for (local, ranges) in window.held.iter().enumerate() {
            let Some(voice) = voices.get(&(index, local)) else {
                continue;
            };
            let counts = held.entry(*voice).or_insert_with(|| vec![0_u16; cells]);
            for (start, end) in ranges {
                for cell in cells_of(*start, *end, cells) {
                    counts[cell] += 1;
                }
            }
        }
    }

    let mut turns = Vec::new();
    for (voice, counts) in held {
        let mut open: Option<usize> = None;
        for cell in 0..cells {
            let active = covers[cell] > 0 && counts[cell] * 2 >= covers[cell];
            match (active, open) {
                (true, None) => open = Some(cell),
                (false, Some(from)) => {
                    turns.push(Turn::new(
                        channel,
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
                channel,
                from as u64 * GRID_MS,
                cells as u64 * GRID_MS,
                voice.index(),
            ));
        }
    }
    turns
}

/// The sub-span of a turn to hold a voice up by: its middle, at most 10 s.
///
/// **Separate from the turn on purpose, and that separation was a
/// measurement finding.** An early version used these rules — the catalog's,
/// for *Voiceprint quality* — to define turns as well, and the close-out's
/// DER came back 38.4% with 28% of speech simply missed. Of course it did:
/// clipping a 15 s turn to its middle 10 s throws away a third of it, which
/// is exactly right for choosing what to embed and exactly wrong for saying
/// who was talking.
///
/// So a turn covers all of its speech, and only the clip is taken from its
/// middle. This now picks the stretch a voice is *played back* from — the
/// same rule, for the same reason: the ends of a long turn are where a
/// neighbour's words bleed in. Returns `None` when there is too little clean
/// voiced audio to stand for a voice — the turn still exists, it just does
/// not get to define one.
pub fn embeddable(start_ms: u64, end_ms: u64) -> Option<(u64, u64)> {
    let length = end_ms.saturating_sub(start_ms);
    if length < MIN_SPAN_MS {
        return None;
    }
    if length <= MAX_SPAN_MS {
        return Some((start_ms, end_ms));
    }
    // The beginning and end of a long turn are where a neighbour's words
    // bleed in, so take the middle.
    let middle = start_ms + length / 2;
    Some((middle - MAX_SPAN_MS / 2, middle + MAX_SPAN_MS / 2))
}

/// The samples one local speaker holds, with everybody else's removed.
///
/// Concatenated rather than zeroed in place: silence dragged through the
/// filterbank pulls an embedding toward "quiet room", and the point of
/// masking is to hand the model one voice and nothing else.
fn gather(samples: &[f32], ranges: &[(u64, u64)]) -> Vec<f32> {
    let index = |ms: u64| (ms as usize * SAMPLE_RATE as usize / 1000).min(samples.len());
    let mut held = Vec::new();
    for (start, end) in ranges {
        let (from, to) = (index(*start), index(*end));
        if to > from {
            held.extend_from_slice(&samples[from..to]);
        }
    }
    held
}

/// Joins consecutive turns of the same voice on the same channel.
fn merge_adjacent(mut turns: Vec<Turn>) -> Vec<Turn> {
    turns.sort_by_key(|turn| (turn.channel == AudioChannel::System, turn.start.millis()));
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
        let mut provisional: BTreeMap<Cluster, Embedding> = BTreeMap::new();
        // Where each provisional vector came from — which channel's window
        // list, which window, which local speaker — so the timeline can be
        // rebuilt once clustering has said which voice it belongs to.
        let mut source: Vec<(usize, usize, usize, Cluster)> = Vec::new();
        let mut seen: Vec<(AudioChannel, Vec<LocalWindow>)> = Vec::new();
        let mut next = 0_u32;
        let mut reached = 0_u64;

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

            let windows = self.segment(samples)?;
            for (index, window) in windows.iter().enumerate() {
                if cancel.is_cancelled() {
                    return Err(DiarizeError::Cancelled);
                }
                for (local, ranges) in window.held.iter().enumerate() {
                    let voiced = held_ms(ranges);
                    if voiced < MIN_EMBED_MS {
                        continue;
                    }
                    let Some(vector) = self.embed(&gather(samples, ranges))? else {
                        continue;
                    };
                    let cluster = Cluster(next);
                    next += 1;
                    provisional.insert(
                        cluster,
                        Embedding::new(vector, &self.model_name, &self.model_version, voiced),
                    );
                    source.push((seen.len(), index, local, cluster));
                }

                // Monotonic across the two channels: a bar that restarted at
                // zero halfway through would read as the job starting over.
                reached = reached.max(window.end_ms.min(total_ms));
                progress(Progress {
                    done_ms: reached,
                    total_ms,
                });
            }
            seen.push((channel, windows));
        }

        // Every window's every local speaker started as its own cluster;
        // grouping them is what turns windows into voices, and it is the only
        // step that knows a slot in one window is the same person as a slot
        // in another.
        let canonical = super::cluster::agglomerate(&provisional);
        let voice_of = |cluster: &Cluster| canonical.get(cluster).copied().unwrap_or(*cluster);

        // One map per channel, in the order the channels were walked, so a
        // window index means something.
        let mut voices: Vec<BTreeMap<(usize, usize), Cluster>> = vec![BTreeMap::new(); seen.len()];
        let mut evidence: BTreeMap<Cluster, Vec<(Vec<f32>, i64, bool)>> = BTreeMap::new();
        for (slot, index, local, cluster) in &source {
            let voice = voice_of(cluster);
            voices[*slot].insert((*index, *local), voice);
            let embedding = &provisional[cluster];
            evidence.entry(voice).or_default().push((
                embedding.vector.clone(),
                embedding.voiced_ms as i64,
                false,
            ));
        }

        let mut turns: Vec<Turn> = seen
            .iter()
            .zip(&voices)
            .flat_map(|((channel, windows), voices)| reconstruct(windows, voices, *channel))
            .collect();
        // Runs separated by less than a breath are one turn. Without this a
        // correct attribution reads as a stutter.
        turns = merge_adjacent(turns);

        // How much of each voice there was, and where best to hear it.
        // Measured on the merged turns rather than summed over windows:
        // windows overlap ninefold, so their total counts every second ten
        // times. The sample is the middle of the longest turn — the stretch
        // least likely to carry a neighbour's words at either end.
        let mut voiced: BTreeMap<Cluster, u64> = BTreeMap::new();
        let mut longest: BTreeMap<Cluster, Turn> = BTreeMap::new();
        for turn in &turns {
            *voiced.entry(turn.cluster).or_default() += turn.duration_ms();
            let best = longest.entry(turn.cluster).or_insert(*turn);
            if turn.duration_ms() > best.duration_ms() {
                *best = *turn;
            }
        }

        let embeddings = evidence
            .into_iter()
            .filter_map(|(cluster, mut observations)| {
                // Longest first, because `centroid` keeps only the last
                // `MAX_EXEMPLARS` it is handed. Left in window order that cap
                // would build a Voiceprint out of the final thirty seconds
                // of the meeting rather than out of its best evidence.
                observations.sort_by_key(|(_, voiced_ms, _)| std::cmp::Reverse(*voiced_ms));
                observations.truncate(super::cluster::MAX_EXEMPLARS);

                let vector = super::cluster::centroid(&observations)?;
                let mut embedding = Embedding::new(
                    vector,
                    &self.model_name,
                    &self.model_version,
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

    /// One window's worth of scripted holds.
    fn window(start_ms: u64, end_ms: u64, held: &[&[(u64, u64)]]) -> LocalWindow {
        let mut rows: Vec<Vec<(u64, u64)>> = vec![Vec::new(); LOCAL_SPEAKERS];
        for (local, ranges) in held.iter().enumerate() {
            rows[local] = ranges.to_vec();
        }
        LocalWindow {
            start_ms,
            end_ms,
            held: rows,
        }
    }

    fn voices(entries: &[((usize, usize), u32)]) -> BTreeMap<(usize, usize), Cluster> {
        entries
            .iter()
            .map(|(key, voice)| (*key, Cluster(*voice)))
            .collect()
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
        assert_eq!(
            POWERSET.iter().flat_map(|set| set.iter()).copied().max(),
            Some(LOCAL_SPEAKERS - 1)
        );
    }

    #[test]
    fn the_window_slides_by_a_tenth_of_itself() {
        // The published 18.8% is measured at this step, and every instant
        // being seen ten times is what the majority vote in `reconstruct`
        // rests on.
        assert_eq!(SEGMENT_STEP * 10, SEGMENT_WINDOW);
    }

    #[test]
    fn the_powerset_keeps_who_was_speaking_not_how_many() {
        // The bug this ticket exists to fix was one line long: the class
        // index was turned into `set.len()` and the identity thrown away.
        let rows = activations(&[0, 1, 4, 6, 2]);
        assert_eq!(rows.len(), LOCAL_SPEAKERS);
        assert_eq!(rows[0], vec![false, true, true, false, false]);
        assert_eq!(rows[1], vec![false, false, true, true, true]);
        assert_eq!(rows[2], vec![false, false, false, true, false]);
    }

    #[test]
    fn an_overlap_gives_both_speakers_the_frame() {
        // Class 4 is speakers 0 and 1 together. The old pipeline read it as
        // "two voices" and produced a turn for neither.
        let rows = activations(&[4]);
        assert!(rows[0][0] && rows[1][0], "both hold it: {rows:?}");
    }

    #[test]
    fn holds_are_placed_on_the_capture_clock() {
        // 10 ms frames, starting 2 s in: speaker 0 holds frames 1 and 2.
        let held = window_hold(&[0, 1, 1, 0], 2_000, 10.0);
        assert_eq!(held[0], vec![(2_010, 2_030)]);
        assert!(held[1].is_empty() && held[2].is_empty());
    }

    #[test]
    fn a_hold_running_to_the_end_of_a_window_is_closed() {
        let held = window_hold(&[1, 1, 1], 0, 10.0);
        assert_eq!(held[0], vec![(0, 30)]);
    }

    // ---- Reconstruction ----

    #[test]
    fn overlapped_speech_is_attributed_to_every_speaker_in_it() {
        // The 27% of AMI speaker-time the old pipeline could not attribute
        // to anybody. Two local speakers hold the same second; both are real
        // people and both get a turn.
        let windows = vec![window(0, 10_000, &[&[(2_000, 6_000)], &[(4_000, 8_000)]])];
        let turns = reconstruct(
            &windows,
            &voices(&[((0, 0), 7), ((0, 1), 9)]),
            AudioChannel::System,
        );

        let at = crate::audio::CaptureOffset(5_000);
        let covering: Vec<_> = turns.iter().filter(|turn| turn.contains(at)).collect();
        assert_eq!(covering.len(), 2, "both voices are speaking: {turns:?}");
        let mut clusters: Vec<_> = covering.iter().map(|turn| turn.cluster).collect();
        clusters.sort_unstable();
        assert_eq!(clusters, vec![Cluster(7), Cluster(9)]);
    }

    #[test]
    fn a_sub_second_interjection_is_attributed() {
        // "Mm-hm" is a real turn by a real person. The old pipeline made
        // turns out of material long enough to embed and so gave it none.
        let windows = vec![window(0, 10_000, &[&[(0, 8_000)], &[(3_000, 3_300)]])];
        let turns = reconstruct(
            &windows,
            &voices(&[((0, 0), 0), ((0, 1), 1)]),
            AudioChannel::Mic,
        );

        let interjection = turns
            .iter()
            .find(|turn| turn.cluster == Cluster(1))
            .expect("the interjection has a speaker");
        assert_eq!(interjection.start.millis(), 3_000);
        assert_eq!(interjection.duration_ms(), 300);
    }

    #[test]
    fn turn_coverage_and_embeddable_span_are_different_questions() {
        // The test that fails if one is ever reused as the other — the
        // ordering of the two floors is asserted at compile time beside
        // them, and this is the behaviour that ordering buys. A 600 ms hold
        // is above the clustering floor, so it gets a vector and therefore a
        // speaker, and below the Voiceprint minimum, so it may not stand for
        // a voice.
        let windows = vec![window(0, 10_000, &[&[(1_000, 1_600)]])];
        let turns = reconstruct(&windows, &voices(&[((0, 0), 0)]), AudioChannel::Mic);
        assert_eq!(turns.len(), 1, "the turn exists");
        assert_eq!(turns[0].duration_ms(), 600);

        assert!(
            held_ms(&[(1_000, 1_600)]) >= MIN_EMBED_MS,
            "and it was embeddable for clustering"
        );
        assert_eq!(
            embeddable(turns[0].start.millis(), turns[0].end.millis()),
            None,
            "but it does not get to define a voice"
        );
    }

    #[test]
    fn an_instant_needs_a_majority_of_the_windows_that_saw_it() {
        // Ten windows see every instant. A boundary two of them place
        // differently is the model wobbling, not a turn.
        let scarce: Vec<LocalWindow> = (0..10)
            .map(|index| {
                let held: &[(u64, u64)] = if index < 2 { &[(4_000, 5_000)] } else { &[] };
                window(0, 10_000, &[held])
            })
            .collect();
        let keys: Vec<((usize, usize), u32)> = (0..10).map(|index| ((index, 0), 0)).collect();
        assert!(
            reconstruct(&scarce, &voices(&keys), AudioChannel::Mic).is_empty(),
            "two windows out of ten is not a turn"
        );

        let plenty: Vec<LocalWindow> = (0..10)
            .map(|index| {
                let held: &[(u64, u64)] = if index < 6 { &[(4_000, 5_000)] } else { &[] };
                window(0, 10_000, &[held])
            })
            .collect();
        let turns = reconstruct(&plenty, &voices(&keys), AudioChannel::Mic);
        assert_eq!(turns.len(), 1, "six out of ten is: {turns:?}");
        assert_eq!(
            (turns[0].start.millis(), turns[0].end.millis()),
            (4_000, 5_000)
        );
    }

    #[test]
    fn a_local_speaker_with_no_vector_contributes_nothing() {
        // A slot too short to embed has no voice to be rebuilt into. It is
        // absent rather than guessed at.
        let windows = vec![window(0, 10_000, &[&[(0, 4_000)], &[(5_000, 5_100)]])];
        let turns = reconstruct(&windows, &voices(&[((0, 0), 0)]), AudioChannel::Mic);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].cluster, Cluster(0));
    }

    #[test]
    fn a_local_slot_is_not_an_identity_across_windows() {
        // Slot 0 in two windows is two different people whenever clustering
        // says so. Anything that stitched slots by index would merge them.
        let windows = vec![
            window(0, 10_000, &[&[(0, 9_000)]]),
            window(10_000, 20_000, &[&[(10_000, 19_000)]]),
        ];
        let turns = reconstruct(
            &windows,
            &voices(&[((0, 0), 0), ((1, 0), 1)]),
            AudioChannel::Mic,
        );
        let mut clusters: Vec<_> = turns.iter().map(|turn| turn.cluster).collect();
        clusters.sort_unstable();
        clusters.dedup();
        assert_eq!(clusters.len(), 2, "two voices, not one: {turns:?}");
    }

    #[test]
    fn silence_produces_no_turns() {
        assert!(reconstruct(&[], &BTreeMap::new(), AudioChannel::Mic).is_empty());
        let quiet = vec![window(0, 10_000, &[])];
        assert!(reconstruct(&quiet, &BTreeMap::new(), AudioChannel::Mic).is_empty());
    }

    #[test]
    fn reconstruction_never_runs_past_the_last_window() {
        let windows = vec![window(0, 2_500, &[&[(0, 2_500)]])];
        let turns = reconstruct(&windows, &voices(&[((0, 0), 0)]), AudioChannel::Mic);
        assert_eq!(turns.len(), 1);
        assert!(
            turns[0].end.millis() <= 2_500,
            "the meeting ends where the audio does: {turns:?}"
        );
    }

    // ---- The oracle floor, which is this ticket's real subject ----

    #[test]
    fn perfect_clustering_now_leaves_almost_nothing_on_the_table() {
        // What the corpus measures at scale, demonstrated on a timeline
        // small enough to check by hand. `oracle_relabel` gives the
        // hypothesis the best possible names, so what is left is purely a
        // failure of *coverage* — speech the pipeline never offered anybody.
        //
        // Alice talks over Bob for two seconds — the 27% of AMI
        // speaker-time that is overlapped — and Carol drops a 300 ms
        // interjection into a pause of Alice's, which is the 6% that is
        // single-speaker and too short. Neither had a representation before.
        use super::super::score;

        let reference = vec![
            score::Span::new("alice", 0, 4_000),
            score::Span::new("carol", 4_000, 4_300),
            score::Span::new("alice", 4_300, 10_000),
            score::Span::new("bob", 8_000, 15_000),
        ];

        // What segmentation sees, and what this pipeline now keeps: three
        // local speakers, each with its own frames.
        let windows = vec![window(
            0,
            15_000,
            &[
                &[(0, 4_000), (4_300, 10_000)],
                &[(8_000, 15_000)],
                &[(4_000, 4_300)],
            ],
        )];
        let turns = merge_adjacent(reconstruct(
            &windows,
            &voices(&[((0, 0), 0), ((0, 1), 1), ((0, 2), 2)]),
            AudioChannel::Mic,
        ));
        let hypothesis: Vec<score::Span> = turns
            .iter()
            .map(|turn| {
                score::Span::new(
                    &turn.cluster.index().to_string(),
                    turn.start.millis(),
                    turn.end.millis(),
                )
            })
            .collect();
        let floor = score::der(&reference, &score::oracle_relabel(&hypothesis, &reference));

        // The version this replaced counted a frame as speech only where one
        // voice held it, merged across gaps under 400 ms, and made turns only
        // from what was long enough to embed. On this timeline that is two
        // spans — and the 300 ms gap around Carol is under the merge gap, so
        // her interjection is swallowed rather than merely dropped.
        let old = vec![
            score::Span::new("0", 0, 8_000),
            score::Span::new("1", 10_000, 15_000),
        ];
        let old_floor = score::der(&reference, &score::oracle_relabel(&old, &reference));

        assert!(
            old_floor.rate() > 0.20,
            "the old floor was the thing worth fixing: {old_floor:?}"
        );
        assert!(
            floor.rate() < old_floor.rate() / 10.0,
            "and the new one is an order of magnitude under it: \
             {floor:?} against {old_floor:?}"
        );
        assert_eq!(
            floor.missed_ms, 0,
            "every reference speaker is offered to somebody, which is the \
             whole of what this ticket changed: {floor:?}"
        );
        // Nothing invented either. Alice's two turns sit either side of a
        // 300 ms pause and the merge gap would ordinarily bridge it, which
        // would credit her with the moment Carol takes; it does not, because
        // Carol's turn is between them in time and `merge_adjacent` only
        // joins neighbours. Attributing the interjection is what protects
        // the pause.
        assert_eq!(floor.false_alarm_ms, 0, "{floor:?}");
    }

    // ---- What a voice may be built from ----

    #[test]
    fn a_long_turn_keeps_its_length_while_its_sample_is_clipped() {
        // The bug this guards against cost 28% of speech in the close-out
        // measurement: clipping the *turn* to the middle 10 s meant two
        // thirds of a long turn had no speaker at all.
        let (start, end) = (0, 60_000);
        let (sample_start, sample_end) = embeddable(start, end).expect("embeddable");
        assert_eq!(sample_end - sample_start, MAX_SPAN_MS);
        assert!(
            sample_start > 20_000,
            "taken from the middle, not the start"
        );
    }

    #[test]
    fn a_turn_shorter_than_the_minimum_defines_no_voice() {
        assert_eq!(embeddable(0, MIN_SPAN_MS - 1), None);
        assert!(embeddable(0, MIN_SPAN_MS).is_some());
    }

    // ---- Merging ----

    #[test]
    fn consecutive_runs_of_one_voice_become_one_turn() {
        // Otherwise a correct attribution reads as a stutter — the same
        // person restarting every few seconds.
        let merged = merge_adjacent(vec![
            Turn::new(AudioChannel::Mic, 0, 3_000, 0),
            Turn::new(AudioChannel::Mic, 3_000, 4_500, 0),
            Turn::new(AudioChannel::Mic, 4_500, 6_000, 0),
        ]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].end.millis(), 6_000);
    }

    #[test]
    fn someone_drawing_breath_is_not_the_end_of_their_turn() {
        let merged = merge_adjacent(vec![
            Turn::new(AudioChannel::Mic, 0, 3_000, 0),
            Turn::new(AudioChannel::Mic, 3_000 + MERGE_GAP_MS, 6_000, 0),
        ]);
        assert_eq!(merged.len(), 1, "a 400 ms gap is a breath");
    }

    #[test]
    fn a_long_silence_does_end_a_turn() {
        let merged = merge_adjacent(vec![
            Turn::new(AudioChannel::Mic, 0, 3_000, 0),
            Turn::new(AudioChannel::Mic, 5_000, 6_000, 0),
        ]);
        assert_eq!(merged.len(), 2);
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

    #[test]
    fn overlapping_turns_of_two_voices_survive_merging() {
        // `merge_adjacent` runs over every reconstructed turn, so it is the
        // last place overlap could quietly be flattened.
        let merged = merge_adjacent(vec![
            Turn::new(AudioChannel::System, 0, 6_000, 0),
            Turn::new(AudioChannel::System, 4_000, 9_000, 1),
        ]);
        assert_eq!(merged.len(), 2);
    }

    // ---- Masking ----

    #[test]
    fn gathering_takes_only_the_frames_a_speaker_holds() {
        // One second of ones, one of twos, one of threes; a speaker holding
        // the middle second must be handed twos and nothing else.
        let mut samples = vec![1.0_f32; SAMPLE_RATE as usize];
        samples.extend(vec![2.0_f32; SAMPLE_RATE as usize]);
        samples.extend(vec![3.0_f32; SAMPLE_RATE as usize]);

        let held = gather(&samples, &[(1_000, 2_000)]);
        assert_eq!(held.len(), SAMPLE_RATE as usize);
        assert!(held.iter().all(|value| *value == 2.0));
    }

    #[test]
    fn gathering_joins_a_speakers_pieces_without_the_gap() {
        let mut samples = vec![1.0_f32; SAMPLE_RATE as usize];
        samples.extend(vec![2.0_f32; SAMPLE_RATE as usize]);
        samples.extend(vec![3.0_f32; SAMPLE_RATE as usize]);

        let held = gather(&samples, &[(0, 1_000), (2_000, 3_000)]);
        assert_eq!(held.len(), 2 * SAMPLE_RATE as usize);
        assert!(
            held.iter().all(|value| *value != 2.0),
            "the other speaker's second is not in there"
        );
    }

    #[test]
    fn gathering_past_the_end_of_the_audio_is_clamped() {
        let samples = vec![1.0_f32; SAMPLE_RATE as usize];
        assert_eq!(gather(&samples, &[(0, 9_000)]).len(), samples.len());
        assert!(gather(&samples, &[(5_000, 9_000)]).is_empty());
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
    fn a_stretch_short_enough_to_be_an_interjection_still_embeds() {
        // The clustering floor is only honest if the model accepts what it
        // lets through. If this fails, `MIN_EMBED_MS` is below what the
        // embedding model can take and every interjection is an error.
        let Some(dir) = model_dir() else {
            eprintln!("skipped: set EVERTRANSCRIPT_DIARIZE_MODELS to run this");
            return;
        };
        let mut diarizer =
            LiveDiarizer::load(&dir.join("segmentation.onnx"), &dir.join("embedding.onnx"))
                .expect("models load");

        let brief = tone(150.0, MIN_EMBED_MS as f32 / 1000.0, 5);
        assert!(
            diarizer.embed(&brief).expect("embeds").is_some(),
            "{MIN_EMBED_MS} ms is the floor this pipeline promises to handle"
        );
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

        let windows = diarizer
            .segment(&vec![0.0_f32; SEGMENT_WINDOW])
            .expect("runs on silence");
        assert_eq!(windows.len(), 1, "one window covers exactly one window");
        assert!(
            windows[0].held.iter().all(|ranges| ranges.is_empty()),
            "silence must contain no speakers: {:?}",
            windows[0]
        );
    }

    #[test]
    fn the_window_slides_across_a_longer_channel() {
        let Some(dir) = model_dir() else {
            eprintln!("skipped: set EVERTRANSCRIPT_DIARIZE_MODELS to run this");
            return;
        };
        let mut diarizer =
            LiveDiarizer::load(&dir.join("segmentation.onnx"), &dir.join("embedding.onnx"))
                .expect("models load");

        // 15 s: one window at 0, then a window every second until the last
        // one reaches the end.
        let windows = diarizer
            .segment(&tone(130.0, 15.0, 6))
            .expect("runs")
            .iter()
            .map(|window| window.start_ms)
            .collect::<Vec<_>>();
        assert_eq!(windows.first(), Some(&0));
        assert_eq!(windows.get(1), Some(&1_000), "a one-second step");
        assert_eq!(windows.last(), Some(&5_000), "the last window ends at 15 s");
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

    #[test]
    fn a_whole_meeting_diarizes_end_to_end() {
        // The only test here that exercises `diarize` rather than a piece of
        // it. Every defect this project has shipped lived in a path nothing
        // executed.
        let Some(dir) = model_dir() else {
            eprintln!("skipped: set EVERTRANSCRIPT_DIARIZE_MODELS to run this");
            return;
        };
        let mut diarizer =
            LiveDiarizer::load(&dir.join("segmentation.onnx"), &dir.join("embedding.onnx"))
                .expect("models load");

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
        // structure has to be coherent.
        for turn in &result.turns {
            assert!(turn.end > turn.start, "a turn with no duration: {turn:?}");
        }
        for cluster in result.clusters() {
            assert!(
                result.embeddings.contains_key(&cluster),
                "cluster {cluster:?} has turns but no voice, which cannot \
                 happen now that every turn came from a vector"
            );
        }
        for (cluster, embedding) in &result.embeddings {
            let own: Vec<&Turn> = result
                .turns
                .iter()
                .filter(|turn| turn.cluster == *cluster)
                .collect();
            let spoken: u64 = own.iter().map(|turn| turn.duration_ms()).sum();
            assert_eq!(
                embedding.voiced_ms, spoken,
                "the voice knows its own length"
            );

            // A voice made only of interjections has nowhere clean to be
            // played back from, and says so rather than offering a clip too
            // short to recognize.
            let longest = own.iter().map(|turn| turn.duration_ms()).max().unwrap_or(0);
            assert_eq!(
                embedding.sample.is_some(),
                longest >= MIN_SPAN_MS,
                "a sample exists exactly when a turn is long enough to hold \
                 the voice up: longest {longest} ms"
            );
            if let Some(sample) = embedding.sample {
                assert!(
                    own.iter().any(|turn| turn.channel == sample.channel
                        && turn.start <= sample.start
                        && sample.end <= turn.end),
                    "the sample lies inside one of the voice's own turns"
                );
            }
        }
    }
}
