//! What the diarization pipeline actually scores, on a public corpus.
//!
//! M3 owed a Diarization Error Rate, produced one, and deleted the scratch
//! that produced it — so 49.7%, the 32.6% oracle floor, and the whole
//! embedding bake-off behind ADR-0037 were claims in a markdown file that
//! nobody could check. This is the same measurement, committed.
//!
//! **It drives the shipped pipeline**, not a copy instrumented for analysis.
//! The close-out's attribution of the gap happened to be trustworthy because
//! its copy produced the same turns on all sixteen meetings; that property
//! is worth having deliberately rather than by luck.
//!
//! **Nothing here runs in the default `cargo test` path, and nothing here
//! reaches the network.** The corpus is fetched by `scripts/fetch-ami.sh`,
//! ahead of time, by a person; this binary reads a directory or skips
//! loudly. AMI's Mix-Headset audio is CC BY 4.0 and BUT's references are
//! Apache-2.0, and neither is committed — together they are several
//! gigabytes, and vendoring a corpus into a repository that promises to be
//! offline would be the worst possible way to keep that promise.
//!
//! ```text
//! scripts/fetch-ami.sh ~/ami
//! EVERTRANSCRIPT_MEASURE_DER=1 EVERTRANSCRIPT_AMI_DIR=~/ami \
//!   cargo test -p evertranscript-core --test diarization_accuracy -- --nocapture
//! ```

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;
use std::time::Instant;

use evertranscript_core::diarize;
use evertranscript_core::diarize::score;
use evertranscript_core::diarize::score::Der;
use evertranscript_core::diarize::score::Span;
use evertranscript_core::diarize::score::Trial;
use evertranscript_core::models::registry::VoiceprintId;

/// Set to run at all. Separate from the corpus path because they answer
/// different questions — "should this machine measure" and "where is the
/// corpus" — and a workflow that sets the second on every platform still
/// wants to set the first on one.
const MEASURE_ENV: &str = "EVERTRANSCRIPT_MEASURE_DER";
/// Where `scripts/fetch-ami.sh` put the corpus.
const CORPUS_ENV: &str = "EVERTRANSCRIPT_AMI_DIR";

/// The floor a Voiceprint has to clear to be a match, as `diarize::cluster`
/// ships it. Duplicated rather than imported because the point of reporting
/// a rate at this number is to notice when the constant and the model have
/// drifted apart — an import would silently follow one of them.
const MATCH_FLOOR: f32 = 0.62;

/// One meeting's worth of corpus.
struct Meeting {
    name: String,
    audio: PathBuf,
    reference: PathBuf,
}

/// The corpus, or a loud skip.
///
/// Set-but-missing fails rather than skips: a CI job that was meant to
/// measure and quietly measured nothing is the failure mode this whole file
/// exists to prevent.
fn corpus() -> Option<Vec<Meeting>> {
    std::env::var_os(MEASURE_ENV).filter(|value| !value.is_empty())?;

    let configured = std::env::var_os(CORPUS_ENV).filter(|value| !value.is_empty());
    let Some(configured) = configured else {
        panic!(
            "{MEASURE_ENV} is set but {CORPUS_ENV} is not. Fetch the corpus first:\n  \
             scripts/fetch-ami.sh <dir>\nthen point {CORPUS_ENV} at <dir>."
        );
    };
    let root = PathBuf::from(configured);
    let audio_dir = root.join("audio");
    let rttm_dir = root.join("rttm");
    assert!(
        audio_dir.is_dir() && rttm_dir.is_dir(),
        "{CORPUS_ENV} points at {}, which has no audio/ and rttm/. Run \
         scripts/fetch-ami.sh to populate it.",
        root.display()
    );

    let mut meetings: Vec<Meeting> = std::fs::read_dir(&rttm_dir)
        .expect("read rttm dir")
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension()? != "rttm" {
                return None;
            }
            let name = path.file_stem()?.to_string_lossy().to_string();
            let audio = audio_dir.join(format!("{name}.wav"));
            audio.exists().then_some(Meeting {
                name,
                audio,
                reference: path,
            })
        })
        .collect();
    meetings.sort_by(|a, b| a.name.cmp(&b.name));

    assert!(
        !meetings.is_empty(),
        "no meeting in {} has both audio and a reference",
        root.display()
    );
    Some(meetings)
}

/// The two ONNX graphs, from wherever this build keeps them.
/// Which embedding to measure, and with which front end.
///
/// **The point of the switch is that only this moves.** Segmentation, turn
/// placement, clustering, the thresholds and the corpus are identical
/// across a pair of runs, so a difference between them is the embedding's.
/// Q115 is why that matters: the bake-off that chose the shipped model ran
/// every candidate through one front end, and it was the wrong one for the
/// model it rejected, so the comparison measured our feature extraction.
///
///   EVERTRANSCRIPT_EMBEDDING=redimnet2   (default) diarize-embedding.onnx
///   EVERTRANSCRIPT_EMBEDDING=wespeaker             diarize-embedding-wespeaker.onnx
///
/// The default is what production ships (ADR-0037 as adopted 2026-09-17);
/// WeSpeaker stays measurable under its own name and file so the record's
/// comparisons can be re-run.
fn embedding_under_test() -> (VoiceprintId, PathBuf) {
    embedding_named(&embedding_name())
}

/// Which embedding the ordinary single-model runs use.
fn embedding_name() -> String {
    std::env::var("EVERTRANSCRIPT_EMBEDDING").unwrap_or_else(|_| "redimnet2".into())
}

/// One named embedding: what its vectors are stamped as — which carries the
/// front end it wants — and which file runs it.
///
/// Split out of [`embedding_under_test`] because the split-model grid needs
/// two of them alive in one process, and both must come from this one table.
/// A second table would be free to disagree about which front end a model
/// takes, and a front end mismatch does not fail — it returns a plausible
/// vector of the wrong thing.
fn embedding_named(name: &str) -> (VoiceprintId, PathBuf) {
    match name {
        // The identity production stamps, so the replay's gallery lookup
        // and the vectors it stores are in one space rather than two that
        // happen to be the same width.
        "redimnet2" => (
            diarize::live::EMBEDDING_IDENTITY,
            "diarize-embedding.onnx".into(),
        ),
        // The model production shipped until 2026-09-17, under the stamp
        // its Voiceprints carry in every History from before the change.
        "wespeaker" => (
            VoiceprintId {
                model: "wespeaker-voxceleb-resnet34-LM",
                version: "2",
                frontend: diarize::live::Frontend::Fbank,
            },
            "diarize-embedding-wespeaker.onnx".into(),
        ),
        other => panic!("EVERTRANSCRIPT_EMBEDDING={other}: expected redimnet2 or wespeaker"),
    }
}

/// How far the segmentation window advances, in milliseconds.
///
/// `EVERTRANSCRIPT_SEGMENT_STEP_MS` overrides it; unset is
/// `diarize::live::SEGMENT_STEP`, what production uses. The step is the
/// one thing that moves in a turn-placement comparison, so the harness
/// prints what it believes it holds — the same discipline
/// `EVERTRANSCRIPT_EMBEDDING` gets, and for the same reason.
fn step_under_test() -> u64 {
    let default = diarize::live::SEGMENT_STEP as u64 * 1000 / diarize::fbank::SAMPLE_RATE as u64;
    match std::env::var("EVERTRANSCRIPT_SEGMENT_STEP_MS") {
        Err(_) => default,
        Ok(value) if value.is_empty() => default,
        Ok(value) => value
            .parse()
            .unwrap_or_else(|_| panic!("EVERTRANSCRIPT_SEGMENT_STEP_MS={value}: expected ms")),
    }
}

/// Where the first segmentation window starts, from
/// `EVERTRANSCRIPT_SEGMENT_PHASE_MS`. Unset is 0, which is production.
///
/// Varies the grid's alignment with its density held fixed, which a step
/// change cannot do: shrinking the step multiplies the windows *and* moves
/// them. Sweeping the phase at one step separates the two.
fn phase_under_test() -> u64 {
    match std::env::var("EVERTRANSCRIPT_SEGMENT_PHASE_MS") {
        Err(_) => 0,
        Ok(value) if value.is_empty() => 0,
        Ok(value) => value
            .parse()
            .unwrap_or_else(|_| panic!("EVERTRANSCRIPT_SEGMENT_PHASE_MS={value}: expected ms")),
    }
}

/// The merge thresholds to score, from `EVERTRANSCRIPT_MERGE_SWEEP` as a
/// comma-separated list.
///
/// Unset is one pass at the shipped [`diarize::cluster::MERGE_THRESHOLD`] —
/// today's behaviour and today's numbers. A threshold is where a similarity
/// distribution becomes a partition, and two embeddings do not put their
/// distributions in the same place, so comparing two models at one number
/// compares a configuration and not the models.
fn thresholds_under_test() -> Vec<f32> {
    let shipped = vec![diarize::cluster::MERGE_THRESHOLD];
    match std::env::var("EVERTRANSCRIPT_MERGE_SWEEP") {
        Err(_) => shipped,
        Ok(value) if value.trim().is_empty() => shipped,
        Ok(value) => value
            .split(',')
            .map(|part| {
                part.trim().parse().unwrap_or_else(|_| {
                    panic!("EVERTRANSCRIPT_MERGE_SWEEP={value}: expected comma-separated numbers")
                })
            })
            .collect(),
    }
}

/// Where the ONNX files live. Harness-side: production has its own.
fn model_directory() -> PathBuf {
    std::env::var_os("EVERTRANSCRIPT_MODELS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(evertranscript_core::paths::models_dir)
}

/// One embedding pass's inputs, resolved against the model directory.
///
/// Carried as one value so a pass cannot be assembled from one model's file
/// and another's stamp — which is the failure the split grid is most exposed
/// to, and the only one of its failures that would not show up as an error.
struct Which {
    id: VoiceprintId,
    path: PathBuf,
}

fn which(name: &str) -> Which {
    let (id, file) = embedding_named(name);
    let path = model_directory().join(&file);
    assert!(
        path.exists(),
        "{name} needs {}, which is not there. Fetch the models with \
         `evertranscript models fetch` first — measuring without them would \
         report a perfect score on an empty hypothesis.",
        path.display()
    );
    Which { id, path }
}

fn models() -> (PathBuf, Which) {
    let dir = model_directory();
    let segmentation = dir.join("diarize-segmentation.onnx");
    let embedding = which(&embedding_name());
    println!(
        "embedding under test: {} v{} ({})",
        embedding.id.model,
        embedding.id.version,
        embedding.path.display()
    );
    println!("segmentation step: {} ms", step_under_test());
    println!(
        "same-window cannot-link: {}",
        if cannot_link_enabled() {
            "enforced"
        } else {
            "off (shipped clusterer)"
        }
    );
    assert!(
        segmentation.exists(),
        "the segmentation model is not in {}. Fetch them with \
         `evertranscript models fetch` first — measuring without them would \
         report a perfect score on an empty hypothesis.",
        dir.display()
    );
    (segmentation, embedding)
}

/// AMI Mix-Headset: one mixed mono channel at 16 kHz.
///
/// It goes on the microphone leg and the system leg stays empty. A corpus
/// meeting has no far end: everyone is in the room, and splitting the mix
/// across both legs would hand the echo canceller a copy of itself to
/// subtract.
fn read_wav(path: &Path) -> Vec<f32> {
    let mut reader = hound::WavReader::open(path).expect("open wav");
    let spec = reader.spec();
    assert_eq!(
        spec.sample_rate,
        diarize::fbank::SAMPLE_RATE,
        "{} is at {} Hz; scripts/fetch-ami.sh resamples to {}",
        path.display(),
        spec.sample_rate,
        diarize::fbank::SAMPLE_RATE
    );
    assert_eq!(spec.channels, 1, "{} is not mono", path.display());

    match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .map(|sample| sample.expect("sample"))
            .collect(),
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|sample| sample.expect("sample") as f32 * scale)
                .collect()
        }
    }
}

/// The pipeline's own answer, as spans on the common timeline.
///
/// Cluster index is the speaker label. The scorer never sees the
/// reference's names, so any labelling that partitions the same way scores
/// the same.
fn hypothesis(turns: &[diarize::Turn]) -> Vec<Span> {
    turns
        .iter()
        .map(|turn| {
            Span::new(
                &format!("cluster-{}", turn.cluster.index()),
                turn.start.millis(),
                turn.end.millis(),
            )
        })
        .collect()
}

struct Measured {
    name: String,
    der: Der,
    oracle: Der,
    seconds: f64,
    audio_seconds: f64,
    embeddings: Vec<(String, Vec<f32>)>,
    /// Pre-merge window vectors, each labelled by the reference speaker who
    /// holds most of it. The raw material for the oracle ceiling.
    windows: Vec<(String, Vec<f32>)>,
    /// Pairs of observations from the same window whose reference speakers
    /// differ, and how many of them agglomeration merged anyway.
    ///
    /// Two local speakers in one window are two people by construction, and
    /// `agglomerate` has no cannot-link constraint to say so. Ticket 03
    /// flagged it and no fixture can catch it, because fixture vectors are
    /// orthogonal and never come close enough to merge.
    cannot_link: (u64, u64, u64),
}

/// The reference speaker holding most of these milliseconds, if any is.
///
/// A window straddling a speaker change belongs to whoever is in most of it;
/// one that is all silence or all overlap belongs to nobody and is dropped,
/// because a vector with no owner cannot say anything about the embedding.
fn dominant_speaker(ranges: &[(u64, u64)], reference: &[Span]) -> Option<String> {
    let mut held: BTreeMap<&str, u64> = BTreeMap::new();
    for (start, end) in ranges {
        for span in reference {
            let overlap = (*end)
                .min(span.end_ms)
                .saturating_sub((*start).max(span.start_ms));
            if overlap > 0 {
                *held.entry(span.speaker.as_str()).or_default() += overlap;
            }
        }
    }
    held.into_iter()
        .max_by_key(|(_, ms)| *ms)
        .map(|(who, _)| who.to_string())
}

/// One unit-length centroid per (meeting, reference speaker), built from every
/// window that speaker owns.
///
/// This is what perfect clustering would hand the matcher, so the accuracy it
/// reaches is the embedding's own ceiling — and the gap between it and the
/// shipped numbers is our clustering's, not the model's. It is the only
/// measurement here that separates the two, which is why an A/B that moves
/// only the embedding needs it.
fn oracle_voices(measured: &[Measured]) -> Vec<(String, Labelled)> {
    measured
        .iter()
        .map(|one| {
            let mut sums: BTreeMap<String, (Vec<f32>, usize)> = BTreeMap::new();
            for (who, vector) in &one.windows {
                let slot = sums
                    .entry(who.clone())
                    .or_insert_with(|| (vec![0.0; vector.len()], 0));
                if slot.0.len() == vector.len() {
                    for (into, from) in slot.0.iter_mut().zip(vector) {
                        *into += from;
                    }
                    slot.1 += 1;
                }
            }
            let voices = sums
                .into_iter()
                .map(|(who, (sum, count))| {
                    let mean: Vec<f32> = sum.iter().map(|v| v / count as f32).collect();
                    let norm = mean.iter().map(|v| v * v).sum::<f32>().sqrt();
                    let unit = if norm == 0.0 {
                        mean
                    } else {
                        mean.iter().map(|v| v / norm).collect()
                    };
                    (who, unit)
                })
                .collect();
            (one.name.clone(), voices)
        })
        .collect()
}

/// Voices labelled by who they belong to, for one meeting.
type Labelled = Vec<(String, Vec<f32>)>;

/// Every cross-meeting pair drawn from labelled voices.
fn cross_meeting_pairs(meetings: &[(String, Labelled)]) -> Vec<Trial> {
    let mut trials = Vec::new();
    for (index, (_, left)) in meetings.iter().enumerate() {
        for (_, right) in &meetings[index + 1..] {
            for (left_name, left_vector) in left {
                for (right_name, right_vector) in right {
                    trials.push(Trial {
                        score: cosine(left_vector, right_vector),
                        same_speaker: left_name == right_name,
                    });
                }
            }
        }
    }
    trials
}

/// One meeting's inference, kept so the clustering that follows can run more
/// than once over it.
///
/// Inference is the whole cost here — 0.02x real time, about a minute a
/// meeting — and the merge threshold is applied long after the models have
/// stopped talking. Sweeping thirteen thresholds therefore costs one pass,
/// not thirteen.
struct Inferred {
    name: String,
    reference: Vec<Span>,
    observed: diarize::live::Observed,
    audio_seconds: f64,
    seconds: f64,
}

/// SHA-256 of a file, hex.
///
/// Never memoized by path: a snapshot's whole job is to notice that an input
/// changed, and a memo keyed on the name would answer for the file that used
/// to be there. Hashing a model or a WAV costs a fraction of a second against
/// an inference pass that costs ten minutes.
fn digest(path: &Path) -> String {
    use sha2::{Digest, Sha256};

    let bytes = std::fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    format!("{:x}", Sha256::digest(&bytes))
}

/// One pass's observations as they go to disk.
///
/// Harness-local and deliberately dull: `Observation` is production's type and
/// is not serialisable, and making it so for a scratch file would be a change
/// to production for a test's convenience.
#[derive(serde::Serialize, serde::Deserialize)]
struct Record {
    stamp: String,
    windows: Vec<(u8, u64, u64)>,
    tracks: Vec<Track>,
    audio_seconds: f64,
    seconds: f64,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Track {
    channel: u8,
    cluster: u32,
    window: usize,
    local: u8,
    vector: Vec<f32>,
    runs: Vec<(u64, u64)>,
    clean_runs: Vec<(u64, u64)>,
}

fn channel_of(slot: u8) -> evertranscript_protocol::AudioChannel {
    if slot == 0 {
        evertranscript_protocol::AudioChannel::Mic
    } else {
        evertranscript_protocol::AudioChannel::System
    }
}

fn slot_of(channel: evertranscript_protocol::AudioChannel) -> u8 {
    u8::from(channel == evertranscript_protocol::AudioChannel::System)
}

/// One meeting's inference, on disk, so a repair to the evaluator does not
/// re-buy it.
///
/// Opt in with `EVERTRANSCRIPT_OBSERVATIONS=<dir>`; unset, nothing is read or
/// written and every run infers as it always did. This is not a cache
/// framework and should not become one: no eviction, no index, no sharing
/// between machines. One file per meeting per embedding, and a provenance
/// stamp that has to match exactly — the audio and both model files **by
/// content**, the corpus and meeting they were found under, the embedding's
/// identity, the front end and the step. Anything else, or a file that will
/// not parse, and the snapshot is ignored out loud and the meeting
/// re-inferred, because silently reusing a pass made from different inputs is
/// the one failure a snapshot can cause that the numbers downstream would not
/// show.
struct Snapshot {
    dir: PathBuf,
    stamp: String,
}

/// Everything that could make two passes over one meeting differ.
///
/// Content, not names. A path says where a file was looked for and nothing
/// about what was there: a re-cut WAV keeps its name, and a ReDimNet2 export
/// once sat under WeSpeaker's filename in this project, which a name-keyed
/// stamp would have read as the same model twice.
fn provenance(meeting: &Meeting, segmentation: &Path, embedding: &Which) -> String {
    format!(
        "evertranscript-observations 2\tcorpus={}\tmeeting={}\taudio={}\tsegmentation={}\t\
         model={}\tversion={}\tembedding={}\tfrontend={:?}\tstep_ms={}\tphase_ms={}",
        meeting.audio.parent().unwrap_or(Path::new("")).display(),
        meeting.name,
        digest(&meeting.audio),
        digest(segmentation),
        embedding.id.model,
        embedding.id.version,
        digest(&embedding.path),
        embedding.id.frontend,
        step_under_test(),
        phase_under_test(),
    )
}

impl Snapshot {
    fn of(meeting: &Meeting, segmentation: &Path, embedding: &Which) -> Option<Self> {
        let dir = PathBuf::from(std::env::var_os("EVERTRANSCRIPT_OBSERVATIONS")?);
        std::fs::create_dir_all(&dir).ok()?;
        Some(Self {
            stamp: provenance(meeting, segmentation, embedding),
            dir,
        })
    }

    fn path(&self, meeting: &Meeting, embedding: &Which) -> PathBuf {
        self.dir.join(format!(
            "{}.{}.{}.observations.json",
            meeting.name, embedding.id.model, embedding.id.version
        ))
    }

    fn load(&self, path: &Path) -> Option<(diarize::live::Observed, f64, f64)> {
        let bytes = std::fs::read(path).ok()?;
        // Truncated, half-written or hand-edited all land here, and all mean
        // the same thing: infer again, and say why.
        let record: Record = match serde_json::from_slice(&bytes) {
            Ok(record) => record,
            Err(error) => {
                println!("  ignoring {}: {error}", path.display());
                return None;
            }
        };
        if record.stamp != self.stamp {
            println!(
                "  ignoring {}: it was made from different inputs",
                path.display()
            );
            return None;
        }
        Some((
            diarize::live::Observed {
                observations: record
                    .tracks
                    .into_iter()
                    .map(|track| diarize::live::Observation {
                        channel: channel_of(track.channel),
                        cluster: diarize::Cluster(track.cluster),
                        window: track.window,
                        local: track.local,
                        vector: track.vector,
                        runs: track.runs,
                        clean_runs: track.clean_runs,
                    })
                    .collect(),
                embedding: diarize::live::EMBEDDING_IDENTITY,
                windows: record
                    .windows
                    .into_iter()
                    .map(|(channel, start, end)| (channel_of(channel), start, end))
                    .collect(),
            },
            record.audio_seconds,
            record.seconds,
        ))
    }

    fn save(&self, path: &Path, one: &Inferred) {
        let record = Record {
            stamp: self.stamp.clone(),
            windows: one
                .observed
                .windows
                .iter()
                .map(|(channel, start, end)| (slot_of(*channel), *start, *end))
                .collect(),
            tracks: one
                .observed
                .observations
                .iter()
                .map(|observation| Track {
                    channel: slot_of(observation.channel),
                    cluster: observation.cluster.index(),
                    window: observation.window,
                    local: observation.local,
                    vector: observation.vector.clone(),
                    runs: observation.runs.clone(),
                    clean_runs: observation.clean_runs.clone(),
                })
                .collect(),
            audio_seconds: one.audio_seconds,
            seconds: one.seconds,
        };
        // Written beside the target and renamed over it, so an interrupted run
        // leaves no half-file for the next one to read as real.
        let partial = path.with_extension("partial");
        let written = serde_json::to_vec(&record)
            .map_err(|error| error.to_string())
            .and_then(|bytes| std::fs::write(&partial, bytes).map_err(|error| error.to_string()))
            .and_then(|()| std::fs::rename(&partial, path).map_err(|error| error.to_string()));
        if let Err(error) = written {
            println!("  could not write {}: {error}", path.display());
            let _ = std::fs::remove_file(&partial);
        }
    }
}

/// Run the models over one meeting.
fn observe_once(meeting: &Meeting, segmentation: &Path, embedding: &Which) -> Inferred {
    let reference =
        score::parse_rttm(&std::fs::read_to_string(&meeting.reference).expect("read reference"));

    let snapshot = Snapshot::of(meeting, segmentation, embedding);
    if let Some(snapshot) = &snapshot
        && let Some((mut observed, audio_seconds, seconds)) =
            snapshot.load(&snapshot.path(meeting, embedding))
    {
        // The stamp is the file's, not the snapshot's: a reused pass must
        // carry the identity of the model that actually made it.
        observed.embedding = embedding.id;
        return Inferred {
            name: meeting.name.clone(),
            reference,
            observed,
            audio_seconds,
            seconds,
        };
    }

    let samples = read_wav(&meeting.audio);
    let audio_seconds = samples.len() as f64 / diarize::fbank::SAMPLE_RATE as f64;

    let mut diarizer =
        diarize::live::LiveDiarizer::load_with(segmentation, &embedding.path, embedding.id)
            .expect("load models")
            .with_step(step_under_test())
            .with_phase(phase_under_test());

    // Wall clock per meeting, because a ceiling is one of the things being
    // fixed: clustering was cubic, and 70 s at 1,259 windows projected to a
    // quarter of an hour for a two-hour meeting.
    let started = Instant::now();
    let observed = diarizer
        .observe(
            diarize::MeetingAudio {
                mic: &samples,
                system: &[],
                sample_rate: diarize::fbank::SAMPLE_RATE,
            },
            &mut |_| {},
            &diarize::Cancel::new(),
        )
        .expect("observe");
    let seconds = started.elapsed().as_secs_f64();

    let one = Inferred {
        name: meeting.name.clone(),
        reference,
        observed,
        audio_seconds,
        seconds,
    };
    // Before anything scores it, so a repair to the evaluator never has to
    // re-buy the inference it is repairing.
    if let Some(snapshot) = &snapshot {
        snapshot.save(&snapshot.path(meeting, embedding), &one);
    }
    one
}

/// Cluster one meeting's observations at a named merge threshold and score it.
///
/// `with_windows` asks for the pre-merge window vectors, which the oracle
/// ceiling is built from. They do not depend on the threshold — they are what
/// the model said before any clustering — so exactly one pass of a sweep
/// keeps them and the oracle is reported once, rather than holding thirteen
/// identical copies of every window in the corpus.
/// How often agglomeration merged two people who were provably talking at
/// the same time.
///
/// Two observations from one segmentation window are two *local* speakers —
/// the model separated them within that window, so they are different people
/// by construction. Nothing in `agglomerate` says so: it has no cannot-link
/// constraint, and ticket 03 flagged that as unchecked. This counts the pairs
/// it could have got wrong and the ones it did, scored against the reference
/// so a merge only counts when the two really are different speakers.
///
/// Assumes windows do not overlap, which holds while `SEGMENT_STEP` equals
/// `SEGMENT_WINDOW`: an observation's first run then sits in exactly one
/// window. At a sliding step this needs the window index carried on the
/// observation instead of recovered from its runs.
/// Whether to enforce segmentation's cannot-link pairs, from
/// `EVERTRANSCRIPT_CANNOT_LINK=1`.
///
/// Off is the shipped clusterer, so a sweep with this unset reproduces the
/// numbers already on record.
fn cannot_link_enabled() -> bool {
    std::env::var("EVERTRANSCRIPT_CANNOT_LINK").as_deref() == Ok("1")
}

/// The one place the harness decides which clusterer to run.
///
/// Scoring and the replay both come through here. They did not always: the
/// replay called `cluster_observed` directly while only scoring honoured the
/// switch, so setting the environment variable measured DER under the
/// constraint and recognition without it, and nothing in the output said so.
/// The flag is a parameter rather than a read of the environment so an
/// offline test can exercise both sides of it.
fn clustered(
    observed: &diarize::live::Observed,
    threshold: f32,
    constrained: bool,
) -> diarize::Diarization {
    if constrained {
        diarize::live::cluster_observed_constrained(observed, threshold)
    } else {
        diarize::live::cluster_observed(observed, threshold)
    }
}

/// One cell of the split-model grid: whose partition, and whose vectors.
///
/// Clustering and identity are two jobs one embedding does today, and they
/// are not the same job — clustering asks whether two windows of *this*
/// meeting are one voice, identity asks whether a voice is the one enrolled
/// months ago. A model can be better at one and worse at the other, and
/// while one model does both there is no way to see it.
///
/// The diagonal — one model doing both — is this with `clustering` and
/// `identity` the same pass. It must reduce to [`clustered`] exactly, not
/// approximately: the splice is then the identity substitution, and
/// [`split_clustered`] makes the calls `cluster_observed` makes, in the
/// order it makes them.
struct Cell<'a> {
    clustering: &'a BTreeMap<String, Inferred>,
    identity: &'a BTreeMap<String, Inferred>,
    /// What the identity vectors are stamped with, and so which gallery the
    /// replay reads. Carried rather than read off a meeting because the
    /// seeds query runs before the first meeting of a replay, when there is
    /// no meeting to read it from.
    stamp: VoiceprintId,
}

impl<'a> Cell<'a> {
    /// One model doing both jobs: the control every split cell is read against.
    fn whole(pass: &'a BTreeMap<String, Inferred>, stamp: VoiceprintId) -> Self {
        Self {
            clustering: pass,
            identity: pass,
            stamp,
        }
    }
}

/// How much identity evidence a cell's geometry actually had behind it.
///
/// A cell is not a clean substitution: the two passes can disagree about
/// which observations exist, and the disagreement is not symmetric. This
/// counts both sides so a cell's coverage is reported rather than assumed,
/// and so a cell with thin support cannot be read as if it had full support.
#[derive(Default, Clone, Copy)]
struct Support {
    /// Clustering observations an identity vector was found for.
    matched: usize,
    /// Clustering observations with no identity vector. They keep their
    /// timing and their cluster — the geometry is the clustering pass's and
    /// is never touched — and contribute nothing to their cluster's
    /// centroid.
    unvectored: usize,
    unvectored_ms: u64,
    /// Identity observations with no clustering counterpart. The partition
    /// is the clustering pass's, so there is no cluster these could belong
    /// to; they are excluded, and counted here rather than dropped quietly.
    unplaced: usize,
    unplaced_ms: u64,
}

impl Support {
    fn add(&mut self, other: Support) {
        self.matched += other.matched;
        self.unvectored += other.unvectored;
        self.unvectored_ms += other.unvectored_ms;
        self.unplaced += other.unplaced;
        self.unplaced_ms += other.unplaced_ms;
    }

    /// The share of the clustering pass's observations an identity vector
    /// was found for. Its denominator is one pass's observations, not the
    /// two passes added together.
    fn coverage(&self) -> f64 {
        let seen = self.matched + self.unvectored;
        if seen == 0 {
            return 1.0;
        }
        self.matched as f64 / seen as f64
    }
}

/// The clustering pass's geometry wearing whatever identity vectors exist
/// for it: a left join, never an intersection.
///
/// **Every one of the clustering pass's observations is kept**, with its
/// window, local track, channel, runs, clean runs and provisional cluster
/// exactly as that pass produced them. Restricting the clustering pass to
/// the tracks both passes produced would be a different experiment: nine
/// observations of 12,649 — 0.8s of voiced time — moved 63.960s of
/// WeSpeaker's wrong-returning time into abstention on AMI dev, because
/// dropping an observation changes the partition and the partition is what
/// the ledger is mostly measuring.
///
/// An observation with no identity vector carries an **empty** vector. That
/// is not a zero-valued embedding, which would be a fabricated direction
/// [`diarize::cluster::centroid`] would average in; it is no contribution at
/// all, which `centroid` skips by `!vector.is_empty()`. A cluster all of
/// whose observations are unvectored therefore gets no centroid, is absent
/// from `Diarization::embeddings` while its turns remain, and reaches the
/// scorer as `Missing::NoEmbedding` — production's own abstention path,
/// not a special case the harness invented.
///
/// Nothing is substituted, padded or inferred from a transcript: a
/// neighbour's vector would put a different voice under this one's name.
fn wearing(
    clustering: &diarize::live::Observed,
    identity: &diarize::live::Observed,
    meeting: &str,
) -> (diarize::live::Observed, Support) {
    assert_eq!(
        clustering.windows, identity.windows,
        "{meeting}: the two embedding passes windowed the audio differently. \
         Segmentation is the same model in both, so this is a rig fault and \
         not a model difference."
    );

    let mut by_track: BTreeMap<(usize, u8), &diarize::live::Observation> = BTreeMap::new();
    for observation in &identity.observations {
        assert!(
            by_track
                .insert((observation.window, observation.local), observation)
                .is_none(),
            "{meeting}: window {} local track {} appears twice in the {} pass",
            observation.window,
            observation.local,
            identity.embedding.model,
        );
    }

    let mut support = Support::default();
    let observations: Vec<diarize::live::Observation> = clustering
        .observations
        .iter()
        .map(|observation| {
            let vector = match by_track.remove(&(observation.window, observation.local)) {
                Some(mate) => {
                    assert_eq!(
                        (mate.channel, &mate.runs, &mate.clean_runs),
                        (
                            observation.channel,
                            &observation.runs,
                            &observation.clean_runs
                        ),
                        "{meeting}: window {} local track {} covers different audio in \
                         the two passes",
                        observation.window,
                        observation.local,
                    );
                    support.matched += 1;
                    mate.vector.clone()
                }
                None => {
                    support.unvectored += 1;
                    support.unvectored_ms += observation.voiced_ms();
                    Vec::new()
                }
            };
            diarize::live::Observation {
                channel: observation.channel,
                cluster: observation.cluster,
                window: observation.window,
                local: observation.local,
                vector,
                runs: observation.runs.clone(),
                clean_runs: observation.clean_runs.clone(),
            }
        })
        .collect();

    for leftover in by_track.values() {
        support.unplaced += 1;
        support.unplaced_ms += leftover.voiced_ms();
    }

    (
        diarize::live::Observed {
            observations,
            embedding: identity.embedding,
            windows: clustering.windows.clone(),
        },
        support,
    )
}

/// One cell's diarization: the clustering pass's partition and turns over
/// whatever identity vectors exist for them.
///
/// The partition is computed from the clustering pass's *own, complete*
/// vectors, so the turns this returns are the ones its same-model control
/// returns — identical, not merely close, and identical whether or not the
/// identity pass covered every observation. That is the point: holding turn
/// placement fixed is what makes a ledger difference attributable to
/// identity alone, and it is why a split cell's DER must equal its
/// clustering control's exactly.
fn split_clustered(
    clustering: &diarize::live::Observed,
    identity: &diarize::live::Observed,
    meeting: &str,
    threshold: f32,
    constrained: bool,
) -> (diarize::Diarization, Support) {
    let provisional = diarize::live::provisional_of(clustering);
    let canonical = if constrained {
        diarize::cluster::agglomerate_constrained(
            &provisional,
            threshold,
            &diarize::live::cannot_link_of(clustering),
        )
    } else {
        diarize::cluster::agglomerate_with(&provisional, threshold)
    };
    let (worn, support) = wearing(clustering, identity, meeting);
    (diarize::live::assemble(&worn, &canonical), support)
}

/// Floors and margins the matcher grid crosses, plus the shipped point.
///
/// Coarse on purpose: a diagnostic of where the matcher's behaviour changes,
/// not a search for an operating point to adopt.
const GRID_FLOORS: [f32; 8] = [0.30, 0.45, 0.55, 0.62, 0.70, 0.80, 0.90, 0.95];
const GRID_MARGINS: [f32; 4] = [0.00, 0.08, 0.15, 0.25];

/// The matcher settings to replay, from `EVERTRANSCRIPT_MATCHER_GRID=1`.
///
/// Unset is the shipped pair alone, so an ordinary replay is the run it
/// always was.
fn matcher_grid() -> Vec<(f32, f32)> {
    // Named points, for a held-out run whose configurations were declared
    // before anyone looked at a held-out number. Not a sweep: the whole
    // value of a declared point is that the split cannot choose it.
    if let Ok(points) = std::env::var("EVERTRANSCRIPT_MATCHER_POINTS") {
        let declared: Vec<(f32, f32)> = points
            .split(',')
            .map(|point| {
                let (floor, margin) = point
                    .split_once(':')
                    .unwrap_or_else(|| panic!("{point}: expected floor:margin"));
                (
                    floor.trim().parse().expect("floor"),
                    margin.trim().parse().expect("margin"),
                )
            })
            .collect();
        assert!(!declared.is_empty(), "no matcher points given");
        return declared;
    }
    if std::env::var("EVERTRANSCRIPT_MATCHER_GRID").as_deref() != Ok("1") {
        return vec![(
            diarize::cluster::MATCH_FLOOR,
            diarize::cluster::MATCH_MARGIN,
        )];
    }
    let mut grid: Vec<(f32, f32)> = GRID_FLOORS
        .iter()
        .flat_map(|floor| GRID_MARGINS.iter().map(move |margin| (*floor, *margin)))
        .collect();
    assert!(
        grid.contains(&(
            diarize::cluster::MATCH_FLOOR,
            diarize::cluster::MATCH_MARGIN
        )),
        "the shipped point has to be in the grid or the grid cannot be read \
         against what production does today"
    );
    grid.sort_by(|a, b| a.partial_cmp(b).expect("no NaN in a literal grid"));
    grid
}

/// How often clustering merged two voices that segmentation had separated.
///
/// **The pairs come from [`diarize::live::cannot_link_of`], not from a second
/// grouping written here.** They were built twice, and the copy in this file
/// drifted: it looked the source window up by geometry while the other read
/// the recorded one, so under a slide the two disagreed about which
/// observations were even comparable. One of them is the constraint the
/// clusterer is given, which makes it the one this has to score against.
///
/// `cannot_link_of` returns both directions, so each pair is taken once.
/// Where the reference names two *different* known people, the pair is a
/// violation if clustering merged it. Where it names the **same** person
/// twice — segmentation split one voice into two local tracks of one window —
/// the constraint forbids a merge that ought to happen, and that is counted
/// separately as `forbidden_rejoins`. Reporting the first without the second
/// would flatter the constraint.
///
/// Both counts are descriptive, and neither is a tally of decisions. A
/// forbidden rejoin is counted whether or not the unconstrained run merged
/// the pair, so most were never a merge to lose; merging is transitive over a
/// cluster, so pairs are not independent; and a dominant label is a majority
/// over a span that may itself be mixed. Do not divide one by the other.
fn same_window_merges(one: &Inferred, threshold: f32) -> (u64, u64, u64) {
    let provisional = diarize::live::provisional_of(&one.observed);
    let forbidden = diarize::live::cannot_link_of(&one.observed);
    // Scored against whichever clusterer actually ran, or a constrained
    // sweep would report the unconstrained violation rate.
    let canonical = if cannot_link_enabled() {
        diarize::cluster::agglomerate_constrained(&provisional, threshold, &forbidden)
    } else {
        diarize::cluster::agglomerate_with(&provisional, threshold)
    };

    // Who the reference says each provisional cluster is. A lookup, not a
    // vote: `observe` numbers a cluster per observation, so there is exactly
    // one observation behind each.
    let who: BTreeMap<diarize::Cluster, String> = one
        .observed
        .observations
        .iter()
        .filter_map(|observation| {
            Some((
                observation.cluster,
                dominant_speaker(&observation.runs, &one.reference)?,
            ))
        })
        .collect();

    let (mut merged, mut pairs, mut rejoins) = (0u64, 0u64, 0u64);
    for (left, rights) in &forbidden {
        for right in rights {
            if right <= left {
                continue;
            }
            let (Some(left_who), Some(right_who)) = (who.get(left), who.get(right)) else {
                continue;
            };
            if left_who == right_who {
                rejoins += 1;
                continue;
            }
            pairs += 1;
            if canonical.get(left) == canonical.get(right) {
                merged += 1;
            }
        }
    }
    (merged, pairs, rejoins)
}

fn score_at(one: &Inferred, threshold: f32, with_windows: bool) -> Measured {
    let reference = &one.reference;
    let result = clustered(&one.observed, threshold, cannot_link_enabled());
    let spans = hypothesis(&result.turns);
    let cannot_link = same_window_merges(one, threshold);

    Measured {
        windows: if with_windows {
            one.observed
                .observations
                .iter()
                .filter_map(|observation| {
                    let who = dominant_speaker(&observation.runs, reference)?;
                    Some((who, observation.vector.clone()))
                })
                .collect()
        } else {
            Vec::new()
        },
        name: one.name.clone(),
        cannot_link,
        der: score::der(reference, &spans),
        oracle: score::der(reference, &score::oracle_relabel(&spans, reference)),
        seconds: one.seconds,
        audio_seconds: one.audio_seconds,
        // Labelled by the reference speaker the cluster mostly is, so a
        // cross-meeting trial knows whether two vectors are the same person.
        //
        // **By summed overlap**, which is what `dominant_speaker` already
        // computes for the oracle windows — so this asks the same question
        // of a cluster that that asks of a window, with one implementation.
        // Two earlier readings were wrong: taking the first relabelled turn
        // named a cluster after whoever opened it, and tallying relabelled
        // turns by their own length awarded a whole turn to the speaker who
        // merely held most of it, so six seconds of Alice inside a ten
        // second turn outvoted seven seconds of Bob spread over two.
        embeddings: result
            .embeddings
            .iter()
            .filter_map(|(cluster, embedding)| {
                let own: Vec<(u64, u64)> = spans
                    .iter()
                    .filter(|span| span.speaker == format!("cluster-{}", cluster.index()))
                    .map(|span| (span.start_ms, span.end_ms))
                    .collect();
                let who = dominant_speaker(&own, reference)?;
                Some((who, embedding.vector.clone()))
            })
            .collect(),
    }
}

fn cosine(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() {
        return 0.0;
    }
    let dot: f32 = left.iter().zip(right).map(|(a, b)| a * b).sum();
    let left_norm: f32 = left.iter().map(|a| a * a).sum::<f32>().sqrt();
    let right_norm: f32 = right.iter().map(|b| b * b).sum::<f32>().sqrt();
    if left_norm == 0.0 || right_norm == 0.0 {
        return 0.0;
    }
    dot / (left_norm * right_norm)
}

/// Every cross-*meeting* pair of voiceprints.
///
/// Across meetings on purpose, and only across. Two vectors from the same
/// meeting share a room, a microphone and a minute of acoustics, so a model
/// that told them apart would prove nothing about the question the product
/// actually asks — which is whether the colleague who spoke last Tuesday is
/// the one speaking now. The negatives here are the same colleagues in the
/// same room, which is the hard case and the real one.
fn cross_meeting_trials(measured: &[Measured]) -> Vec<Trial> {
    let mut trials = Vec::new();
    for (index, left) in measured.iter().enumerate() {
        for right in &measured[index + 1..] {
            for (left_name, left_vector) in &left.embeddings {
                for (right_name, right_vector) in &right.embeddings {
                    trials.push(Trial {
                        score: cosine(left_vector, right_vector),
                        same_speaker: left_name == right_name,
                    });
                }
            }
        }
    }
    trials
}

/// Everything one clustering configuration scores, printed.
///
/// `with_oracle` prints the ceiling block, which is built from the pre-merge
/// windows and so is the same at every merge threshold — printing it thirteen
/// times would suggest it were being measured thirteen times. Returns the
/// pooled and oracle rates so the caller can assert on them.
fn report(measured: &[Measured], with_oracle: bool) -> (Der, Der) {
    for one in measured {
        // Per meeting, so one bad meeting is visible rather than averaged
        // into the corpus figure.
        println!(
            "{:<12}  DER {:>6.1}%  (missed {:>5.1}  false alarm {:>5.1}  confusion {:>5.1})  \
             oracle {:>6.1}%",
            one.name,
            one.der.rate() * 100.0,
            one.der.missed_rate() * 100.0,
            one.der.false_alarm_rate() * 100.0,
            one.der.confusion_rate() * 100.0,
            one.oracle.rate() * 100.0,
        );
    }

    // Pooled, not averaged: a ninety-second meeting must not weigh as much
    // as a fifty-minute one, and pyannote's published figure is pooled.
    let mut pooled = Der::default();
    let mut oracle = Der::default();
    for one in measured {
        pooled.accumulate(&one.der);
        oracle.accumulate(&one.oracle);
    }

    let trials = cross_meeting_trials(measured);
    println!("\n{} meetings", measured.len());
    println!(
        "DER            {:>6.2}%   missed {:.2}  false alarm {:.2}  confusion {:.2}",
        pooled.rate() * 100.0,
        pooled.missed_rate() * 100.0,
        pooled.false_alarm_rate() * 100.0,
        pooled.confusion_rate() * 100.0
    );
    println!(
        "oracle floor   {:>6.2}%   what perfect clustering would still cost",
        oracle.rate() * 100.0
    );
    let merged: u64 = measured.iter().map(|one| one.cannot_link.0).sum();
    let pairs: u64 = measured.iter().map(|one| one.cannot_link.1).sum();
    println!(
        "same-window    {:>6.2}%   {merged} of {pairs} pairs of distinct local tracks from one \
         source window were merged anyway",
        if pairs == 0 {
            0.0
        } else {
            merged as f64 / pairs as f64 * 100.0
        }
    );
    // What the constraint costs to buy that rate: same-window pairs the
    // reference says are one person, which it forbids rejoining.
    let rejoins: u64 = measured.iter().map(|one| one.cannot_link.2).sum();
    println!(
        "forbidden      {:>6.2}%   {rejoins} of {} same-window pairs are one person split in two",
        if pairs + rejoins == 0 {
            0.0
        } else {
            rejoins as f64 / (pairs + rejoins) as f64 * 100.0
        },
        pairs + rejoins
    );
    // **Fragmentation-confounded. Not for ranking configurations.**
    //
    // These two are computed over every provisional voice the run produced,
    // and how many of those there are is itself a function of the
    // configuration: a smaller step, a different merge threshold or a
    // constraint all change the count and the size of what each voice was
    // built from. A configuration that shatters one person into six
    // fragments is scored on six easy near-duplicate pairs. So these move
    // with fragmentation as much as with recognition, and two configurations
    // cannot be ordered by them — which was done once (Q210) and withdrawn
    // (Q211). The oracle block below is the one that answers the recognition
    // question, because its voices are one per person per meeting whatever
    // the clustering did.
    println!("-- all-pairs, fragmentation-confounded; do not rank configurations on these --");
    match score::equal_error_rate(&trials) {
        Some((rate, threshold)) => println!(
            "cross-meeting  EER {:>5.2}% at {threshold:.3}   {} trials",
            rate * 100.0,
            trials.len()
        ),
        None => println!("cross-meeting  EER not computable: one class is empty"),
    }
    if let Some(rate) = score::false_accept_rate_at(&trials, MATCH_FLOOR) {
        println!(
            "at MATCH_FLOOR {MATCH_FLOOR}: {:>5.1}% of different colleagues would be \
             accepted as the same person",
            rate * 100.0
        );
    }
    // The shipped voiceprints, each against every other meeting's: does the
    // right person win?
    let shipped: Vec<(String, Labelled)> = measured
        .iter()
        .map(|one| (one.name.clone(), one.embeddings.clone()))
        .collect();
    let shipped_candidates: Vec<score::Candidate> = shipped
        .iter()
        .flat_map(|(meeting, voices)| {
            voices.iter().map(move |(who, vector)| score::Candidate {
                group: meeting,
                speaker: who,
                vector,
            })
        })
        .collect();
    match score::nearest_is_right(&shipped_candidates) {
        Some(rate) => println!(
            "nearest voice  {:>5.1}% right   {} voices, each against every other meeting's",
            rate * 100.0,
            shipped_candidates.len()
        ),
        None => println!("nearest voice  not askable: one meeting's voices have nobody to meet"),
    }
    println!("-- end fragmentation-confounded block --");

    if with_oracle {
        // The ceiling. One centroid per person per meeting, built from the
        // reference rather than from our clustering, so what it reaches is
        // what the embedding can do and the gap below it is ours to close.
        let oracle_voices = oracle_voices(measured);
        let oracle_candidates: Vec<score::Candidate> = oracle_voices
            .iter()
            .flat_map(|(meeting, voices)| {
                voices.iter().map(move |(who, vector)| score::Candidate {
                    group: meeting,
                    speaker: who,
                    vector,
                })
            })
            .collect();
        let oracle_pairs = cross_meeting_pairs(&oracle_voices);
        println!(
            "\noracle voices  {} voices from perfect clustering, one per person per \
             WHOLE meeting — a ceiling, not this product's recognition answer",
            oracle_candidates.len()
        );
        match score::equal_error_rate(&oracle_pairs) {
            Some((rate, threshold)) => println!(
                "  cross-meeting EER {:.2}% at {threshold:.3}   {} trials",
                rate * 100.0,
                oracle_pairs.len()
            ),
            None => println!("  cross-meeting EER not computable"),
        }
        match score::nearest_is_right(&oracle_candidates) {
            Some(rate) => println!("  nearest voice     {:.1}% right", rate * 100.0),
            None => println!("  nearest voice     not askable"),
        }
        match score::refusal_point(&oracle_pairs) {
            Some((threshold, refused)) => println!(
                "  floor that admits nobody: {threshold:.3}, refusing {:.2}% of genuine pairs",
                refused * 100.0
            ),
            None => println!("  floor that admits nobody: no impostor pairs to refuse"),
        }
    }

    (pooled, oracle)
}

#[test]
fn the_pipeline_scores_what_the_record_says_it_scores() {
    let Some(meetings) = corpus() else {
        eprintln!(
            "skipping: {MEASURE_ENV} is not set. This is the DER and EER harness \
             behind ADR-0037; it needs the AMI corpus, which is fetched by \
             scripts/fetch-ami.sh and never by this test."
        );
        return;
    };
    let (segmentation, embedding) = models();

    let thresholds = thresholds_under_test();
    let sweeping = thresholds.len() > 1;

    // Inference once per meeting, clustering once per threshold over the same
    // observations. A thirteen-point sweep then costs one pass, and every
    // point in it saw byte-identical model output — so a difference between
    // two thresholds is the threshold's and nothing else's.
    let mut by_threshold: Vec<Vec<Measured>> = (0..thresholds.len()).map(|_| Vec::new()).collect();
    for meeting in &meetings {
        let inferred = observe_once(meeting, &segmentation, &embedding);
        println!(
            "{:<12}  observed {:>6.1}s for {:>6.1}s of audio  ({:.2}x)",
            inferred.name,
            inferred.seconds,
            inferred.audio_seconds,
            inferred.seconds / inferred.audio_seconds.max(1.0),
        );
        for (index, threshold) in thresholds.iter().enumerate() {
            by_threshold[index].push(score_at(&inferred, *threshold, index == 0));
        }
    }

    let total_seconds: f64 = by_threshold[0].iter().map(|one| one.seconds).sum();
    let total_audio: f64 = by_threshold[0].iter().map(|one| one.audio_seconds).sum();

    for (index, threshold) in thresholds.iter().enumerate() {
        if sweeping {
            println!("\n───────── merge threshold {threshold:.2} ─────────");
        }
        let (pooled, oracle) = report(&by_threshold[index], index == 0);

        // Bounds, not targets. The point of this binary is the numbers it
        // prints; asserting the recorded figure exactly would fail on the
        // first honest improvement, and asserting nothing would let a
        // pipeline that silently stopped producing turns pass. So: it ran,
        // it produced something, and the floor is below the real thing.
        assert!(pooled.total_ms > 0, "no reference speech was scored at all");
        assert!(
            oracle.rate() <= pooled.rate() + 1e-9,
            "the oracle floor is above the real rate at threshold {threshold}, \
             which cannot happen: {oracle:?} vs {pooled:?}"
        );
        assert!(
            pooled.rate() < 1.0,
            "the pipeline attributed nothing usable at threshold {threshold}: {pooled:?}"
        );
    }

    println!(
        "\nwall clock     {total_seconds:.1}s of inference for {total_audio:.1}s of audio  \
         ({:.2}x real time), {} threshold(s) scored from it",
        total_seconds / total_audio.max(1.0),
        thresholds.len()
    );
}

// ======================= chronological enrollment replay =======================
//
// Cross-meeting recognition, measured the way the product meets it: meetings
// arrive one at a time, each resolved against the gallery the ones before it
// built, and what it learns is available to the next.
//
// This replaces numbers that were confounded. Nearest-voice-right and
// cross-meeting EER compare every voice against every other, which rewards
// fragmentation — at a high merge threshold nothing merges, so every cluster
// is a tiny pure fragment that trivially matches its own speaker, and both
// improve while DER collapses (Q140). Two models that fragment differently
// cannot be compared that way at all.
//
// **The unit of score is the segment, not the person.** An earlier version of
// this file chose one representative cluster per reference speaker with
// `optimal_mapping` and gave that person's whole speech time that cluster's
// outcome, which turns 900 seconds right and 100 wrong into 1000 of one or
// the other. Recognition is scored here from the SpeakerID production
// actually stored on each segment, so a person split across two identities
// is reported split. No oracle mapping enters recognition scoring at all.

/// Which manifest to replay, as a path. Unset skips, like the corpus.
const REPLAY_ENV: &str = "EVERTRANSCRIPT_REPLAY_MANIFEST";

/// One meeting in the replay order.
struct Chapter {
    order: usize,
    meeting: String,
    /// Returning events here do not score. Set for IB4002, whose speaker
    /// labels are permuted against IB4001's: only FIE038 is shared any
    /// further, so a different-label neighbour above 0.8 for all four can
    /// only be the partner meeting, and nothing model-free says which of the
    /// pair is wrong. The meeting stays in the chronology, in DER and as an
    /// open-set probe; only these four people's returning seconds stop
    /// scoring, and they stay out of every scored denominator.
    unscorable_returns: bool,
    speakers: Vec<String>,
}

fn load_manifest(path: &Path, corpus: &[Meeting]) -> Vec<Chapter> {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let mut chapters: Vec<Chapter> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let order: usize = parts.next().expect("order").parse().expect("order");
        let meeting = parts.next().expect("meeting").to_string();
        let unscorable_returns = match parts.next().expect("flags") {
            "." => false,
            "unscorable-returns" => true,
            other => panic!("{}: unknown flag {other}", path.display()),
        };
        let speakers: Vec<String> = parts.map(str::to_string).collect();
        assert_eq!(
            order,
            chapters.len() + 1,
            "manifest order is not contiguous"
        );
        chapters.push(Chapter {
            order,
            meeting,
            unscorable_returns,
            speakers,
        });
    }

    // Checked against the corpus rather than trusted: the manifest's whole job
    // is to say who is who, and one that had drifted from the RTTMs would
    // answer that wrongly and silently.
    for chapter in &chapters {
        let found = corpus
            .iter()
            .find(|one| one.name == chapter.meeting)
            .unwrap_or_else(|| panic!("{} is in the manifest but not the corpus", chapter.meeting));
        let mut actual: Vec<String> =
            score::parse_rttm(&std::fs::read_to_string(&found.reference).expect("rttm"))
                .into_iter()
                .map(|span| span.speaker)
                .collect();
        actual.sort();
        actual.dedup();
        assert_eq!(
            actual, chapter.speakers,
            "{}: the manifest and the RTTM disagree about who is in the room",
            chapter.meeting
        );
    }
    assert_eq!(
        chapters.len(),
        corpus.len(),
        "the manifest and the corpus hold different numbers of meetings"
    );
    chapters
}

/// What the evaluator decided a stored Speaker *is*, fixed when it was minted.
///
/// A stored identity that began as Alice must not become "correctly Bob"
/// because later erroneous updates dragged its centroid toward Bob. Ownership
/// is settled once, from the reference speech time of the cluster that minted
/// it, and every later judgement is against that. Lives entirely outside the
/// store: production never sees it.
struct Anchor {
    who: String,
    purity: f64,
    minted_in: String,
}

impl Anchor {
    /// No majority owner. Every later attach to it scores wrong, because
    /// there is no person it could be right about — and it is not a genuine
    /// enrollment of anybody, so it cannot make somebody enrolled-before.
    fn mixed(&self) -> bool {
        self.purity <= 0.5
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Population {
    /// Spoke in an earlier meeting of this order — whether or not either
    /// system managed to enrol them. Reference decides this, not the store.
    Returning,
    New,
}

/// Why production stored no SpeakerID on a segment.
///
/// Read from the assignment and the embeddings rather than guessed from
/// duration: a short cluster that was *recognized* keeps its identity, so
/// voiced time alone does not establish a mint-floor refusal.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Missing {
    /// No turn covered the segment's midpoint, so reconcile chose no cluster.
    NoTurn,
    /// A cluster, but with too little clean voiced audio to embed honestly,
    /// so `persist` never saw it.
    NoEmbedding,
    /// Heard, embedded, unrecognized, and under [`MIN_SPEAKER_MS`] — the only
    /// way `persist` declines to mint under the current rules.
    MintFloor,
    /// Heard and embedded and over the floor, yet nothing was stored. Not a
    /// case the rules produce; reported rather than filed under one of the
    /// above, because filing it would hide a change in `persist`.
    Unexpected,
}

impl Missing {
    fn tag(self) -> &'static str {
        match self {
            Self::NoTurn => "unattributed:no-turn",
            Self::NoEmbedding => "unattributed:no-embedding",
            Self::MintFloor => "unattributed:mint-floor",
            Self::Unexpected => "unattributed:unexpected",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Correct,
    Wrong,
    /// A new anonymous Speaker for somebody the gallery already held.
    AbstainEnrolled,
    /// A new anonymous Speaker for somebody never enrolled — the persist
    /// policy's failure rather than the matcher's refusal.
    AbstainNeverEnrolled,
    CorrectNew,
    FalseAttach,
    /// No SpeakerID at all, with the reason production's own signals give.
    Unattributed(Missing),
    Unscorable,
}

impl Outcome {
    fn tag(self) -> &'static str {
        match self {
            Self::Correct => "correct",
            Self::Wrong => "wrong",
            Self::AbstainEnrolled => "abstain-enrolled",
            Self::AbstainNeverEnrolled => "abstain-never-enrolled",
            Self::CorrectNew => "correct-new",
            Self::FalseAttach => "false-attach",
            Self::Unattributed(why) => why.tag(),
            Self::Unscorable => "unscorable",
        }
    }

    fn parse(tag: &str) -> Self {
        match tag {
            "correct" => Self::Correct,
            "wrong" => Self::Wrong,
            "abstain-enrolled" => Self::AbstainEnrolled,
            "abstain-never-enrolled" => Self::AbstainNeverEnrolled,
            "correct-new" => Self::CorrectNew,
            "false-attach" => Self::FalseAttach,
            "unattributed:no-turn" => Self::Unattributed(Missing::NoTurn),
            "unattributed:no-embedding" => Self::Unattributed(Missing::NoEmbedding),
            "unattributed:mint-floor" => Self::Unattributed(Missing::MintFloor),
            "unattributed:unexpected" => Self::Unattributed(Missing::Unexpected),
            "unscorable" => Self::Unscorable,
            other => panic!("unknown outcome {other}"),
        }
    }

    fn scored(self) -> bool {
        self != Self::Unscorable
    }
}

/// One (meeting, person, outcome) with the reference speech time it holds.
///
/// Several rows per person per meeting, because a person's seconds can land
/// on more than one outcome — which is the whole point of scoring segments.
#[derive(Clone)]
struct Event {
    order: usize,
    meeting: String,
    who: String,
    population: Population,
    outcome: Outcome,
    seconds: f64,
}

/// Replays one split through one embedding, into a store of its own.
///
/// **One store, every meeting, one declared order.** Not a gallery per
/// series: that is not what a user's store looks like, it would hide the
/// cross-series returners AMI has — FIE038 is in both IB and IS1008 — and on
/// a split where every series is a closed four-person group it would leave no
/// impostor pressure at all.
///
/// **The contamination is the policy under test, not a confound.** `persist`
/// folds an accepted match back in as an exemplar with no confirmation gate,
/// so a wrong name can damage later matching. Running with that on is the
/// point. It does make the result depend on this history, which the report
/// has to say.
///
/// **Reference-transcript speaker-time.** One segment per reference turn,
/// identical for both models, with reference-derived boundaries and no ASR.
/// There is no recognizer here and `persist` will not mint for a cluster that
/// owns no words, so something has to stand in for the Transcript. Scoring
/// reads the SpeakerID production stored on each of those segments, so what
/// is measured is the real assignment over reference boundaries — a more
/// generous transcript than a real one, and every number inherits that.
fn replay(
    chapters: &[Chapter],
    cell: &Cell,
    threshold: f32,
    constrained: bool,
    floor: f32,
    margin: f32,
) -> (Vec<Event>, BTreeMap<String, Anchor>) {
    use evertranscript_core::store::meetings;
    use evertranscript_core::store::schema;
    use evertranscript_core::store::speakers;

    let directory = std::env::temp_dir().join(format!(
        "evertranscript-replay-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).expect("replay store directory");
    let mut connection =
        rusqlite::Connection::open(directory.join("replay.db")).expect("replay store");
    schema::configure(&connection).expect("configure");
    schema::migrate(&mut connection).expect("migrate");

    let mut events = Vec::new();
    let mut anchors: BTreeMap<String, Anchor> = BTreeMap::new();
    let mut seen_before: BTreeSet<String> = BTreeSet::new();

    for chapter in chapters {
        let clustering = cell
            .clustering
            .get(&chapter.meeting)
            .expect("every chapter was inferred before the grid started");
        let identity = cell
            .identity
            .get(&chapter.meeting)
            .expect("every chapter was inferred before the grid started");
        let (diarization, _) = split_clustered(
            &clustering.observed,
            &identity.observed,
            &chapter.meeting,
            threshold,
            constrained,
        );
        let channel = clustering
            .observed
            .windows
            .first()
            .map(|&(channel, _, _)| channel)
            .expect("a meeting with no windows");

        // The gallery as it stands *before* this meeting, since persist is
        // about to change it. A mixed anchor is nobody's enrollment, so it
        // cannot make a person enrolled-before.
        let enrolled_before: BTreeSet<String> =
            diarize::cluster::seeds(&connection, cell.stamp.model, cell.stamp.version)
                .expect("seeds")
                .iter()
                .filter_map(|seed| anchors.get(&seed.speaker_id))
                .filter(|anchor| !anchor.mixed())
                .map(|anchor| anchor.who.clone())
                .collect();
        let known_speakers: BTreeSet<String> = speakers::list(&connection)
            .expect("speakers")
            .into_iter()
            .map(|speaker| speaker.id)
            .collect();

        let meeting_id = meetings::start(&connection, Some(&chapter.meeting), None)
            .expect("start")
            .id;
        // Evaluator-only: which reference person each segment is, and how
        // long. Never read by production; this is the denominator.
        let mut owner: BTreeMap<String, (String, u64)> = BTreeMap::new();
        for span in &clustering.reference {
            let segment = meetings::append_segment(
                &connection,
                &meeting_id,
                channel,
                span.start_ms as i64,
                span.end_ms as i64,
                "reference turn",
            )
            .expect("append segment");
            owner.insert(
                segment.id,
                (span.speaker.clone(), span.end_ms - span.start_ms),
            );
        }

        let segments = meetings::segments(&connection, &meeting_id).expect("segments");
        let reconciliation = diarize::reconcile::reconcile(&diarization, &segments);
        let chose: BTreeMap<&str, Option<diarize::Cluster>> = reconciliation
            .assignments
            .iter()
            .map(|one| (one.segment_id.as_str(), one.cluster))
            .collect();
        let assigned = diarize::cluster::persist_with(
            &connection,
            &meeting_id,
            &diarization.embeddings,
            &reconciliation.voices(),
            None,
            &diarize::cluster::Rebuilt::default(),
            floor,
            margin,
        )
        .expect("persist");
        diarize::reconcile::apply(
            &connection,
            &reconciliation,
            &assigned,
            speakers::Attribution::Clustered,
        )
        .expect("apply");
        speakers::sweep_unreferenced(&connection).expect("sweep");
        meetings::set_diarized(&connection, &meeting_id).expect("set diarized");

        // Anchor every Speaker this meeting minted, before scoring anything
        // against it. The oracle mapping lives here and only here: deciding
        // what a *stored identity* is, once, is not the same question as
        // deciding whether a segment was attributed correctly.
        let spans = hypothesis(&diarization.turns);
        for (cluster, speaker_id) in &assigned {
            if known_speakers.contains(speaker_id) || anchors.contains_key(speaker_id) {
                continue;
            }
            let owned: Vec<(u64, u64)> = spans
                .iter()
                .filter(|span| span.speaker == format!("cluster-{}", cluster.index()))
                .map(|span| (span.start_ms, span.end_ms))
                .collect();
            let (who, purity) = ownership(&owned, &clustering.reference);
            anchors.insert(
                speaker_id.clone(),
                Anchor {
                    who,
                    purity,
                    minted_in: chapter.meeting.clone(),
                },
            );
        }

        // Seconds by (person, outcome), read off what production stored.
        let mut held: BTreeMap<(String, &'static str), (Outcome, f64)> = BTreeMap::new();
        for segment in &segments {
            let (who, duration_ms) = owner.get(&segment.id).expect("every segment has an owner");
            let population = if seen_before.contains(who) {
                Population::Returning
            } else {
                Population::New
            };
            let stored =
                speakers::attributed_speaker(&connection, &segment.id).expect("attribution");

            let outcome = classify(
                Scoring {
                    population,
                    unscorable_returns: chapter.unscorable_returns,
                    meeting: &chapter.meeting,
                    who,
                    stored: stored.as_deref(),
                    anchors: &anchors,
                    enrolled_before: &enrolled_before,
                },
                || {
                    why_missing(
                        chose.get(segment.id.as_str()).copied().flatten(),
                        &diarization,
                        &assigned,
                    )
                },
            );

            let slot = held
                .entry((who.clone(), outcome.tag()))
                .or_insert((outcome, 0.0));
            slot.1 += *duration_ms as f64 / 1000.0;
        }

        for ((who, _), (outcome, seconds)) in held {
            let population = if seen_before.contains(&who) {
                Population::Returning
            } else {
                Population::New
            };
            events.push(Event {
                order: chapter.order,
                meeting: chapter.meeting.clone(),
                who,
                population,
                outcome,
                seconds,
            });
        }

        for who in &chapter.speakers {
            seen_before.insert(who.clone());
        }
    }

    let _ = std::fs::remove_dir_all(&directory);
    (events, anchors)
}

/// Why no SpeakerID, from production's own signals rather than from duration.
fn why_missing(
    chosen: Option<diarize::Cluster>,
    diarization: &diarize::Diarization,
    assigned: &BTreeMap<diarize::Cluster, String>,
) -> Missing {
    let Some(cluster) = chosen else {
        return Missing::NoTurn;
    };
    let Some(embedding) = diarization.embeddings.get(&cluster) else {
        return Missing::NoEmbedding;
    };
    if assigned.contains_key(&cluster) {
        // The cluster has an identity but this segment carries none, which
        // `apply` does not do. Surfaced rather than explained away.
        return Missing::Unexpected;
    }
    // Heard — it owns this segment — and embedded, so persist saw it and
    // declined. Recognition has no floor, so declining means unrecognized,
    // and the only remaining gate is the mint floor.
    if embedding.voiced_ms < diarize::cluster::MIN_SPEAKER_MS {
        Missing::MintFloor
    } else {
        Missing::Unexpected
    }
}

/// Who holds most of these milliseconds in the reference, and what share.
///
/// The share is the purity the anchor records: a cluster nobody holds a
/// majority of has no person it could later be right about.
fn ownership(ranges: &[(u64, u64)], reference: &[Span]) -> (String, f64) {
    let mut held: BTreeMap<&str, u64> = BTreeMap::new();
    for (start, end) in ranges {
        for span in reference {
            let overlap = (*end)
                .min(span.end_ms)
                .saturating_sub((*start).max(span.start_ms));
            if overlap > 0 {
                *held.entry(span.speaker.as_str()).or_default() += overlap;
            }
        }
    }
    let total: u64 = held.values().sum();
    match held.into_iter().max_by_key(|(_, ms)| *ms) {
        Some((who, ms)) if total > 0 => (who.to_string(), ms as f64 / total as f64),
        _ => (String::from("(none)"), 0.0),
    }
}

/// Everything one segment's verdict depends on, so the rule is one function
/// with no store behind it and can be checked offline.
struct Scoring<'a> {
    population: Population,
    unscorable_returns: bool,
    meeting: &'a str,
    who: &'a str,
    /// The SpeakerID production actually wrote on this segment.
    stored: Option<&'a str>,
    anchors: &'a BTreeMap<String, Anchor>,
    enrolled_before: &'a BTreeSet<String>,
}

/// One segment's outcome. `missing` is only consulted when nothing was
/// stored, so the caller does not pay to work out a reason it will not use.
fn classify(at: Scoring<'_>, missing: impl FnOnce() -> Missing) -> Outcome {
    if at.unscorable_returns && at.population == Population::Returning {
        return Outcome::Unscorable;
    }
    let Some(id) = at.stored else {
        return Outcome::Unattributed(missing());
    };
    let anchor = at
        .anchors
        .get(id)
        .expect("a stored Speaker is anchored before anything scores against it");
    let minted_here = anchor.minted_in == at.meeting;
    match at.population {
        Population::Returning if minted_here => {
            if at.enrolled_before.contains(at.who) {
                Outcome::AbstainEnrolled
            } else {
                Outcome::AbstainNeverEnrolled
            }
        }
        Population::Returning => {
            if !anchor.mixed() && anchor.who == at.who {
                Outcome::Correct
            } else {
                Outcome::Wrong
            }
        }
        Population::New if minted_here => Outcome::CorrectNew,
        Population::New => Outcome::FalseAttach,
    }
}

/// Grid events as a file: the same rows, with the settings that produced
/// them, so one file carries every configuration.
///
/// Separate from [`write_events`] rather than replacing it, because the
/// pairing reads that format and the runs already on disk are in it.
fn write_grid_events(path: &Path, rows: &[(f32, f32, Event)]) {
    let mut out =
        String::from("floor\tmargin\torder\tmeeting\twho\tpopulation\toutcome\tseconds\n");
    for (floor, margin, event) in rows {
        out.push_str(&format!(
            "{floor:.2}\t{margin:.2}\t{}\t{}\t{}\t{}\t{}\t{:.3}\n",
            event.order,
            event.meeting,
            event.who,
            match event.population {
                Population::Returning => "returning",
                Population::New => "new",
            },
            event.outcome.tag(),
            event.seconds
        ));
    }
    std::fs::write(path, out).expect("write grid events");
}

/// One line per configuration, so 32 of them stay readable.
fn ledger_line(floor: f32, margin: f32, events: &[Event]) {
    let mut totals: BTreeMap<(Population, &str), f64> = BTreeMap::new();
    for event in events {
        *totals
            .entry((event.population, event.outcome.tag()))
            .or_default() += event.seconds;
    }
    let get =
        |population: Population, tag: &str| totals.get(&(population, tag)).copied().unwrap_or(0.0);
    println!(
        "  floor {floor:.2} margin {margin:.2}   returning correct {:8.0} wrong {:7.0} \
         abstain {:7.0} unattributed {:7.0}   new correct {:7.0} false-attach {:6.0}",
        get(Population::Returning, "correct"),
        get(Population::Returning, "wrong"),
        get(Population::Returning, "abstain-enrolled")
            + get(Population::Returning, "abstain-never-enrolled"),
        get(Population::Returning, "unattributed:mint-floor")
            + get(Population::Returning, "unattributed:no-turn")
            + get(Population::Returning, "unattributed:no-embedding")
            + get(Population::Returning, "unattributed:unexpected"),
        get(Population::New, "correct-new"),
        get(Population::New, "false-attach"),
    );
}

/// Events as a file, so two runs pair without either being re-run.
fn write_events(path: &Path, events: &[Event]) {
    let mut out = String::from("order\tmeeting\twho\tpopulation\toutcome\tseconds\n");
    for event in events {
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{:.3}\n",
            event.order,
            event.meeting,
            event.who,
            match event.population {
                Population::Returning => "returning",
                Population::New => "new",
            },
            event.outcome.tag(),
            event.seconds,
        ));
    }
    std::fs::write(path, out).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    println!("events written to {}", path.display());
}

fn read_events(path: &Path) -> Vec<Event> {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    text.lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let field: Vec<&str> = line.split('\t').collect();
            assert_eq!(field.len(), 6, "{}: malformed row {line}", path.display());
            Event {
                order: field[0].parse().expect("order"),
                meeting: field[1].to_string(),
                who: field[2].to_string(),
                population: match field[3] {
                    "returning" => Population::Returning,
                    "new" => Population::New,
                    other => panic!("unknown population {other}"),
                },
                outcome: Outcome::parse(field[4]),
                seconds: field[5].parse().expect("seconds"),
            }
        })
        .collect()
}

/// One person in one meeting: the denominator, and where their seconds went.
struct Ledger {
    population: Population,
    seconds: f64,
    by_outcome: BTreeMap<&'static str, f64>,
}

/// Folds events into per-person ledgers, refusing a file that cannot be one.
fn ledgers(events: &[Event], source: &str) -> BTreeMap<(usize, String, String), Ledger> {
    let mut out: BTreeMap<(usize, String, String), Ledger> = BTreeMap::new();
    let mut seen: BTreeSet<(usize, String, String, &str)> = BTreeSet::new();
    for event in events {
        let key = (event.order, event.meeting.clone(), event.who.clone());
        assert!(
            seen.insert((
                event.order,
                event.meeting.clone(),
                event.who.clone(),
                event.outcome.tag()
            )),
            "{source}: {} {} {} appears twice with outcome {}",
            event.order,
            event.meeting,
            event.who,
            event.outcome.tag()
        );
        let slot = out.entry(key).or_insert_with(|| Ledger {
            population: event.population,
            seconds: 0.0,
            by_outcome: BTreeMap::new(),
        });
        assert!(
            slot.population == event.population,
            "{source}: {} {} changes population between rows",
            event.meeting,
            event.who
        );
        slot.seconds += event.seconds;
        *slot.by_outcome.entry(event.outcome.tag()).or_default() += event.seconds;
    }
    out
}

/// What one replay produced, by population, weighted by reference speech time.
///
/// Unscorable seconds are reported on their own and are in no scored
/// denominator: they are time the corpus cannot adjudicate, not time anybody
/// got wrong.
fn report_replay(events: &[Event], anchors: &BTreeMap<String, Anchor>) {
    for (label, population) in [
        ("returning", Population::Returning),
        ("new", Population::New),
    ] {
        let mine: Vec<&Event> = events
            .iter()
            .filter(|event| event.population == population)
            .collect();
        let scored: f64 = mine
            .iter()
            .filter(|event| event.outcome.scored())
            .map(|event| event.seconds)
            .sum();
        let held_out: f64 = mine
            .iter()
            .filter(|event| !event.outcome.scored())
            .map(|event| event.seconds)
            .sum();
        let people: BTreeSet<&str> = mine.iter().map(|event| event.who.as_str()).collect();
        println!(
            "\n{label}  {} person-meetings over {} people, {scored:.0}s scored\
             {}",
            mine.iter()
                .map(|event| (&event.meeting, &event.who))
                .collect::<BTreeSet<_>>()
                .len(),
            people.len(),
            if held_out > 0.0 {
                format!(", {held_out:.0}s unscorable and out of every denominator")
            } else {
                String::new()
            }
        );
        let mut by_outcome: BTreeMap<&str, f64> = BTreeMap::new();
        for event in &mine {
            *by_outcome.entry(event.outcome.tag()).or_default() += event.seconds;
        }
        for (tag, held) in by_outcome {
            let share = if scored > 0.0 {
                held / scored * 100.0
            } else {
                0.0
            };
            if tag == "unscorable" {
                println!("  {tag:<28} {held:>8.0}s   (not in the denominator)");
            } else {
                println!("  {tag:<28} {held:>8.0}s  {share:>5.1}%");
            }
        }
    }

    // FIE038 is the only person returning past the unverified IB4001/IB4002
    // pair, so its later returns carry a history nothing model-free can check.
    let tainted: Vec<&Event> = events
        .iter()
        .filter(|event| {
            event.who == "FIE038"
                && event.population == Population::Returning
                && event.outcome.scored()
        })
        .collect();
    if !tainted.is_empty() {
        println!("\nFIE038's later returns — history includes the unverified IB4001/IB4002 pair");
        for event in tainted {
            println!(
                "  {:>2} {:<9} {:>7.0}s  {}",
                event.order,
                event.meeting,
                event.seconds,
                event.outcome.tag()
            );
        }
    }

    let mut per_person: BTreeMap<&str, usize> = BTreeMap::new();
    let mut mixed = 0;
    for anchor in anchors.values() {
        *per_person.entry(anchor.who.as_str()).or_default() += 1;
        if anchor.mixed() {
            mixed += 1;
        }
    }
    println!(
        "\nstored identities  {} for {} people ({:.1} each), {mixed} anchored to nobody in particular",
        anchors.len(),
        per_person.len(),
        anchors.len() as f64 / per_person.len().max(1) as f64,
    );
}

/// Pairs two replays and reports the differences descriptively.
///
/// **No significance test.** The unit is the person-meeting, and they are
/// dependent through a shared gallery and a cascading history — a sign test
/// needs independence just as much as a t-test does, so it would be the same
/// error in cheaper clothing. Differences are described, and every one is
/// listed so a reader can go and look.
fn pair_replays(left: &Path, right: &Path) {
    let a = ledgers(&read_events(left), &left.display().to_string());
    let b = ledgers(&read_events(right), &right.display().to_string());

    // Unequal coverage means the two runs did not see the same corpus, and
    // every aggregate below would be comparing different denominators.
    let only_left: Vec<_> = a.keys().filter(|key| !b.contains_key(*key)).collect();
    let only_right: Vec<_> = b.keys().filter(|key| !a.contains_key(*key)).collect();
    assert!(
        only_left.is_empty() && only_right.is_empty(),
        "the two runs cover different person-meetings: {} only on the left, {} only on the right",
        only_left.len(),
        only_right.len()
    );

    let good = ["correct", "correct-new"];
    let bad = ["wrong", "false-attach"];
    let sum = |ledger: &Ledger, tags: &[&str]| -> f64 {
        tags.iter()
            .filter_map(|tag| ledger.by_outcome.get(*tag))
            .sum()
    };

    println!("\ndifferences — {} vs {}", left.display(), right.display());
    let (mut differ, mut same) = (0usize, 0usize);
    let mut totals: BTreeMap<(&str, &str), (f64, f64)> = BTreeMap::new();
    for (key, one) in &a {
        let other = &b[key];
        assert_eq!(
            one.population == Population::Returning,
            other.population == Population::Returning,
            "{} {}: the runs disagree about whether this person is returning",
            key.1,
            key.2
        );
        assert!(
            (one.seconds - other.seconds).abs() < 0.05,
            "{} {}: {:.3}s on the left and {:.3}s on the right — the denominator moved",
            key.1,
            key.2,
            one.seconds,
            other.seconds
        );
        let label = if one.population == Population::Returning {
            "returning"
        } else {
            "new"
        };
        for (tag, pick) in [("correct", &good[..]), ("wrong", &bad[..])] {
            let slot = totals.entry((label, tag)).or_default();
            slot.0 += sum(one, pick);
            slot.1 += sum(other, pick);
        }
        if one.by_outcome == other.by_outcome {
            same += 1;
            continue;
        }
        differ += 1;
        println!(
            "  {:>2} {:<9} {:<10} {:>7.0}s   {:<38} vs {}",
            key.0,
            key.1,
            key.2,
            one.seconds,
            describe(&one.by_outcome),
            describe(&other.by_outcome)
        );
    }

    println!("\n{same} person-meetings identical, {differ} differ");
    for ((label, tag), (left_s, right_s)) in totals {
        println!(
            "{label:<10} {tag:<8} seconds   left {left_s:>8.0}   right {right_s:>8.0}   ({:+.0})",
            right_s - left_s
        );
    }
    println!(
        "\nDescriptive only. These person-meetings share a gallery and a\n\
         cascading history, so they are not independent trials and no\n\
         significance test over them — sign test included — would be valid.\n\
         Equal totals would mean a difference this sample could not resolve."
    );
}

fn describe(by_outcome: &BTreeMap<&'static str, f64>) -> String {
    by_outcome
        .iter()
        .map(|(tag, seconds)| format!("{tag} {seconds:.0}s"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Offline, and the only part of this file that runs in a plain `cargo test`.
///
/// The switch has to reach the replay, not only the scorer.
///
/// It did not: the replay called `cluster_observed` directly, so a run with
/// `EVERTRANSCRIPT_CANNOT_LINK=1` measured DER under the constraint and
/// recognition without it, silently. Both call sites now go through
/// `clustered`, and this is what fails if one of them stops.
#[test]
fn the_constrained_path_is_the_one_the_replay_gets() {
    use evertranscript_protocol::AudioChannel;

    let voice = |cluster: u32, at: u64, vector: Vec<f32>| diarize::live::Observation {
        channel: AudioChannel::Mic,
        cluster: diarize::Cluster(cluster),
        // Two local speakers of the one window, which is what this is about.
        window: 0,
        local: cluster as u8,
        vector,
        runs: vec![(at, at + 4_000)],
        clean_runs: vec![(at, at + 4_000)],
    };
    // Two local speakers of one window, close enough that an unconstrained
    // merge at this threshold takes them for one voice.
    let observed = diarize::live::Observed {
        embedding: diarize::live::EMBEDDING_IDENTITY,
        observations: vec![
            voice(0, 1_000, vec![1.0, 0.10, 0.0]),
            voice(1, 2_000, vec![1.0, -0.10, 0.0]),
        ],
        windows: vec![(AudioChannel::Mic, 0, 10_000)],
    };

    let free = clustered(&observed, 0.5, false);
    let held = clustered(&observed, 0.5, true);
    assert_eq!(
        free.embeddings.len(),
        1,
        "unconstrained, these two are one voice at this threshold"
    );
    assert_eq!(
        held.embeddings.len(),
        2,
        "constrained, one window's two local speakers stay two voices"
    );
}

/// The same-window diagnostic counts pairs by the window that *produced* them.
///
/// It used to find the window by geometry — the first one containing a run's
/// first instant — which is right only while windows tile. Under a slide an
/// instant sits in several, and the earliest match is generally not the
/// source: here the third track's run lies inside the first window as well as
/// its own, and geometry would file it under the first and invent a pair out
/// of two tracks that were never heard together.
///
/// Reading the recorded window is what makes the denominator mean "pairs of
/// distinct local tracks segmentation emitted from one source window" — which
/// is what it emitted, not a claim that the two were speaking at the same
/// instant. This pins that it does, in both
/// clustering modes, because a violation rate scored against the wrong
/// clusterer is no better than one counted over the wrong pairs.
#[test]
fn the_same_window_diagnostic_counts_the_window_an_observation_came_from() {
    use evertranscript_protocol::AudioChannel;

    let track = |window: usize, local: u8, cluster: u32, at: u64, vector: Vec<f32>| {
        diarize::live::Observation {
            channel: AudioChannel::Mic,
            cluster: diarize::Cluster(cluster),
            window,
            local,
            vector,
            runs: vec![(at, at + 3_000)],
            clean_runs: vec![(at, at + 3_000)],
        }
    };
    let observed = diarize::live::Observed {
        embedding: diarize::live::EMBEDDING_IDENTITY,
        observations: vec![
            // Two local tracks of the first window: one real pair.
            track(0, 0, 0, 1_000, vec![1.0, 0.10, 0.0]),
            track(0, 1, 1, 2_000, vec![1.0, -0.10, 0.0]),
            // The second window's tracks. Their runs sit inside the first
            // window too, which is the whole trap.
            track(1, 0, 2, 6_000, vec![0.0, 0.0, 1.0]),
            // One person segmentation split in two: a pair the constraint
            // forbids that ought to be merged.
            track(1, 1, 3, 6_500, vec![0.0, 0.05, 1.0]),
        ],
        // Overlapping, as a slide makes them.
        windows: vec![
            (AudioChannel::Mic, 0, 10_000),
            (AudioChannel::Mic, 5_000, 15_000),
        ],
    };
    let speaking = |who: &str, start: u64, end: u64| Span {
        speaker: who.to_string(),
        start_ms: start,
        end_ms: end,
    };
    let one = Inferred {
        name: "overlapping".to_string(),
        reference: vec![
            speaking("alice", 1_000, 4_000),
            speaking("bob", 2_000, 5_000),
            speaking("carol", 6_000, 9_000),
        ],
        observed,
        audio_seconds: 15.0,
        seconds: 1.0,
    };

    let (merged, pairs, rejoins) = same_window_merges(&one, 0.5);
    assert_eq!(
        pairs, 1,
        "one window held two tracks, so there is one pair to judge; \
         grouping by geometry would find two"
    );
    // Alice and Bob are near enough to be one voice at this threshold, which
    // is the violation — unless the constraint the diagnostic scores against
    // is the one forbidding it.
    let expected = if cannot_link_enabled() { 0 } else { 1 };
    assert_eq!(
        merged, expected,
        "the diagnostic scores the clusterer that ran"
    );
    // The other half of the ledger: the second window's two tracks are both
    // Carol, so forbidding them is a cost, not a violation caught.
    assert_eq!(
        rejoins, 1,
        "a same-window pair the reference calls one person is counted as a \
         rejoin the constraint forbids, never as a pair it judges"
    );
}

/// The snapshot has to give back exactly what inference produced, including
/// the things a record is easy to lose: the channel, a track whose vector is
/// empty, and runs that differ from clean runs.
#[test]
fn a_snapshot_returns_the_pass_that_was_paid_for() {
    let (dir, snapshot, one) = a_saved_pass("round-trip");
    let path = dir.join("round-trip.json");

    let (back, audio_seconds, seconds) = snapshot.load(&path).expect("reload");
    assert_eq!(back.windows, one.observed.windows);
    assert_eq!(back.observations.len(), 2);
    for (got, want) in back.observations.iter().zip(&one.observed.observations) {
        assert_eq!(got.channel, want.channel);
        assert_eq!(got.cluster, want.cluster);
        assert_eq!((got.window, got.local), (want.window, want.local));
        assert_eq!(got.vector, want.vector, "f32s survive the record exactly");
        assert_eq!(got.runs, want.runs);
        assert_eq!(got.clean_runs, want.clean_runs);
    }
    assert_eq!((audio_seconds, seconds), (1234.5, 67.25));

    // Different inputs, same filename: refused rather than silently reused.
    let other = Snapshot {
        dir: dir.clone(),
        stamp: "stamp two".into(),
    };
    assert!(other.load(&path).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A truncated or malformed file is a miss, not a crash.
///
/// The half-written case is what `save`'s rename is there to prevent, but a
/// full disk or a killed run elsewhere can still leave one, and a harness that
/// panicked on it would lose the pass it was trying to protect.
#[test]
fn a_half_written_snapshot_is_a_miss_and_not_a_panic() {
    let (dir, snapshot, _) = a_saved_pass("truncated");
    let path = dir.join("truncated.json");

    let whole = std::fs::read(&path).expect("read back");
    assert!(snapshot.load(&path).is_some(), "whole, it loads");

    std::fs::write(&path, &whole[..whole.len() / 2]).expect("truncate");
    assert!(snapshot.load(&path).is_none(), "half a record is no record");

    std::fs::write(&path, b"").expect("empty");
    assert!(snapshot.load(&path).is_none());

    std::fs::write(&path, b"{\"stamp\": \"stamp one\"}").expect("wrong shape");
    assert!(
        snapshot.load(&path).is_none(),
        "a stamp alone is not a pass: the fields it lacks are the observations"
    );

    assert!(snapshot.load(&dir.join("never-written.json")).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The same WAV path holding different audio is a different meeting.
///
/// This is the failure the stamp exists for and the one a path-keyed record
/// cannot see: a corpus re-cut in place keeps every name it had.
#[test]
fn audio_that_changed_under_the_same_name_invalidates_the_snapshot() {
    let dir = scratch("audio-content");
    let audio = dir.join("ES2011c.wav");
    let segmentation = dir.join("segmentation.onnx");
    let model = dir.join("embedding.onnx");
    std::fs::write(&segmentation, b"segmentation bytes").expect("write");
    std::fs::write(&model, b"embedding bytes").expect("write");

    let meeting = Meeting {
        name: "ES2011c".into(),
        audio: audio.clone(),
        reference: dir.join("ES2011c.rttm"),
    };
    let which = Which {
        id: VoiceprintId {
            model: "wespeaker",
            version: "1",
            frontend: diarize::live::Frontend::Fbank,
        },
        path: model.clone(),
    };

    std::fs::write(&audio, b"the audio as it was").expect("write");
    let before = provenance(&meeting, &segmentation, &which);
    std::fs::write(&audio, b"the audio as it is now").expect("rewrite in place");
    let after = provenance(&meeting, &segmentation, &which);
    assert_ne!(
        before, after,
        "same path, different bytes: the stamp has to move"
    );

    // And the same holds for the models, which is the trap that actually
    // happened here — one export sitting under another's filename.
    std::fs::write(&model, b"a different export entirely").expect("rewrite in place");
    assert_ne!(after, provenance(&meeting, &segmentation, &which));

    // A snapshot written before the change is refused after it.
    let snapshot = Snapshot {
        dir: dir.clone(),
        stamp: before,
    };
    let path = dir.join("stale.json");
    snapshot.save(&path, &a_pass());
    let now = Snapshot {
        dir: dir.clone(),
        stamp: provenance(&meeting, &segmentation, &which),
    };
    assert!(now.load(&path).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(test)]
fn scratch(what: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("et-obs-{}-{what}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// One meeting's pass, with the shapes worth round-tripping in it.
#[cfg(test)]
fn a_pass() -> Inferred {
    use evertranscript_protocol::AudioChannel;

    Inferred {
        name: "ES2011c".into(),
        reference: Vec::new(),
        observed: diarize::live::Observed {
            embedding: diarize::live::EMBEDDING_IDENTITY,
            observations: vec![
                diarize::live::Observation {
                    channel: AudioChannel::System,
                    cluster: diarize::Cluster(7),
                    window: 29,
                    local: 2,
                    vector: vec![0.25, -0.5, 1.0, f32::MIN_POSITIVE, 0.1],
                    runs: vec![(1_000, 2_000), (3_000, 4_500)],
                    clean_runs: vec![(1_200, 1_900)],
                },
                // The left join's own case: a track with no identity vector.
                diarize::live::Observation {
                    channel: AudioChannel::Mic,
                    cluster: diarize::Cluster(0),
                    window: 0,
                    local: 0,
                    vector: Vec::new(),
                    runs: vec![(0, 84)],
                    clean_runs: Vec::new(),
                },
            ],
            windows: vec![(AudioChannel::Mic, 0, 10_000), (AudioChannel::System, 5, 7)],
        },
        audio_seconds: 1234.5,
        seconds: 67.25,
    }
}

#[cfg(test)]
fn a_saved_pass(what: &str) -> (PathBuf, Snapshot, Inferred) {
    let dir = scratch(what);
    let snapshot = Snapshot {
        dir: dir.clone(),
        stamp: "stamp one".into(),
    };
    let one = a_pass();
    snapshot.save(&dir.join(format!("{what}.json")), &one);
    (dir, snapshot, one)
}

/// Two passes over one meeting, differing only in their vectors.
///
/// Offline: it is the splice that is under test here, not either model.
#[cfg(test)]
fn two_passes() -> (diarize::live::Observed, diarize::live::Observed) {
    use evertranscript_protocol::AudioChannel;

    let voice = |cluster: u32, window: usize, local: u8, at: u64, vector: Vec<f32>| {
        diarize::live::Observation {
            channel: AudioChannel::Mic,
            cluster: diarize::Cluster(cluster),
            window,
            local,
            vector,
            runs: vec![(at, at + 4_000)],
            clean_runs: vec![(at, at + 4_000)],
        }
    };
    let windows = vec![
        (AudioChannel::Mic, 0, 10_000),
        (AudioChannel::Mic, 10_000, 20_000),
    ];
    let clustering = diarize::live::Observed {
        embedding: diarize::live::EMBEDDING_IDENTITY,
        observations: vec![
            voice(0, 0, 0, 1_000, vec![1.0, 0.0, 0.0]),
            voice(1, 0, 1, 2_000, vec![0.0, 1.0, 0.0]),
            voice(2, 1, 0, 11_000, vec![1.0, 0.05, 0.0]),
        ],
        windows: windows.clone(),
    };
    // Same audio, same tracks, different vectors — and its clusters
    // deliberately numbered the other way round, because the splice must key
    // on the track and never on the provisional cluster id.
    let identity = diarize::live::Observed {
        embedding: VoiceprintId {
            model: "redimnet2-b3",
            version: "1",
            frontend: diarize::live::Frontend::Waveform,
        },
        observations: vec![
            voice(2, 1, 0, 11_000, vec![0.0, 0.0, 1.0]),
            voice(1, 0, 1, 2_000, vec![0.0, -1.0, 0.0]),
            voice(0, 0, 0, 1_000, vec![-1.0, 0.0, 0.0]),
        ],
        windows,
    };
    (clustering, identity)
}

/// The diagonal cell is not merely close to the control — it is the control.
///
/// A split grid whose same-model cells did not reproduce the numbers already
/// on record would measure the rewrite as well as the split, and there would
/// be no way afterwards to say which of the two moved a ledger.
#[test]
fn a_cell_that_splits_nothing_is_the_run_that_already_ran() {
    let (observed, _) = two_passes();
    for constrained in [false, true] {
        let control = clustered(&observed, 0.5, constrained);
        let (cell, support) = split_clustered(&observed, &observed, "m", 0.5, constrained);
        assert_eq!(
            (support.unvectored, support.unplaced),
            (0, 0),
            "constrained={constrained}: a pass must cover itself completely"
        );
        assert_eq!(
            cell.turns, control.turns,
            "constrained={constrained}: the diagonal placed different turns"
        );
        assert_eq!(
            cell.embeddings.len(),
            control.embeddings.len(),
            "constrained={constrained}: the diagonal found a different partition"
        );
        for (cluster, embedding) in &control.embeddings {
            assert_eq!(
                cell.embeddings[cluster].vector, embedding.vector,
                "constrained={constrained}: the diagonal moved a vector"
            );
        }
    }
}

/// A split cell keeps the partition's turns and changes only whose vectors
/// stand behind them.
#[test]
fn a_split_cell_holds_the_partition_and_swaps_the_vectors() {
    let (clustering, identity) = two_passes();
    let control = clustered(&clustering, 0.5, false);
    let (cell, _) = split_clustered(&clustering, &identity, "m", 0.5, false);

    assert_eq!(
        cell.turns, control.turns,
        "the identity vectors reached turn placement, which they must not"
    );
    for embedding in cell.embeddings.values() {
        assert_eq!(
            (embedding.model.as_str(), embedding.model_version.as_str()),
            ("redimnet2-b3", "1"),
            "a cell's vectors must be stamped with the model that made them"
        );
    }
    let moved = control
        .embeddings
        .iter()
        .any(|(cluster, embedding)| cell.embeddings[cluster].vector != embedding.vector);
    assert!(moved, "the splice substituted nothing");
}

/// Three observations on three windows, and an identity pass that covers
/// only the first of them and adds one of its own.
///
/// The first two merge into one voice at this threshold; the third is a
/// voice on its own with no identity evidence at all, which is the case the
/// left join has to get right and no corpus is needed to pin.
#[cfg(test)]
fn unequal_support() -> (diarize::live::Observed, diarize::live::Observed) {
    use evertranscript_protocol::AudioChannel;

    let voice =
        |cluster: u32, window: usize, at: u64, vector: Vec<f32>| diarize::live::Observation {
            channel: AudioChannel::Mic,
            cluster: diarize::Cluster(cluster),
            window,
            local: 0,
            vector,
            runs: vec![(at, at + 4_000)],
            clean_runs: vec![(at, at + 4_000)],
        };
    let windows: Vec<(AudioChannel, u64, u64)> = (0..4)
        .map(|w| (AudioChannel::Mic, w * 10_000, (w + 1) * 10_000))
        .collect();
    let clustering = diarize::live::Observed {
        embedding: diarize::live::EMBEDDING_IDENTITY,
        observations: vec![
            voice(0, 0, 1_000, vec![1.0, 0.0, 0.0]),
            voice(1, 1, 11_000, vec![1.0, 0.05, 0.0]),
            voice(2, 2, 21_000, vec![0.0, 1.0, 0.0]),
        ],
        windows: windows.clone(),
    };
    let identity = diarize::live::Observed {
        embedding: VoiceprintId {
            model: "redimnet2-b3",
            version: "1",
            frontend: diarize::live::Frontend::Waveform,
        },
        observations: vec![
            // Covers window 0 only, and offers a window the partition has
            // no place for.
            voice(0, 0, 1_000, vec![0.0, 0.0, 1.0]),
            voice(9, 3, 31_000, vec![0.0, 0.0, 1.0]),
        ],
        windows,
    };
    (clustering, identity)
}

/// An observation the identity pass never produced keeps its turn and lends
/// no vector — it does not shrink the partition, and it does not borrow one.
///
/// Restricting the clustering pass to the tracks both produced was the first
/// version of this, and it is a different experiment: on AMI dev nine such
/// observations, 0.8s of voiced time, moved 63.960s of WeSpeaker's
/// wrong-returning time into abstention, because dropping an observation
/// changes the partition.
#[test]
fn a_track_the_identity_pass_missed_keeps_its_turn_and_lends_no_vector() {
    let (clustering, identity) = unequal_support();
    let control = clustered(&clustering, 0.5, false);
    let (cell, support) = split_clustered(&clustering, &identity, "m", 0.5, false);

    assert_eq!(
        cell.turns, control.turns,
        "thin identity support must not move a single turn"
    );
    assert_eq!(
        (support.matched, support.unvectored, support.unplaced),
        (1, 2, 1),
        "the join must count both kinds of disagreement"
    );
    assert!(
        (support.coverage() - 1.0 / 3.0).abs() < 1e-9,
        "coverage is over the clustering pass's own observations, not the two added together: {}",
        support.coverage()
    );

    // The voice that keeps a vector keeps only the one it really has: the
    // first window's, not an average with a substitute for the second.
    let (_, kept) = cell
        .embeddings
        .iter()
        .next()
        .expect("the covered voice still has an embedding");
    assert_eq!(kept.vector, vec![0.0, 0.0, 1.0]);
}

/// A voice with no identity evidence at all keeps its turns and gets no
/// embedding, which is production's own abstention path rather than one the
/// harness invented.
#[test]
fn a_voice_with_no_identity_vector_at_all_is_left_without_an_embedding() {
    let (clustering, identity) = unequal_support();
    let control = clustered(&clustering, 0.5, false);
    let (cell, _) = split_clustered(&clustering, &identity, "m", 0.5, false);

    assert_eq!(
        control.embeddings.len(),
        2,
        "the same-model control has both voices"
    );
    assert_eq!(
        cell.embeddings.len(),
        1,
        "the voice the identity pass never saw must not be given a fabricated vector"
    );
    let voices: BTreeSet<diarize::Cluster> = cell.turns.iter().map(|turn| turn.cluster).collect();
    assert_eq!(
        voices.len(),
        2,
        "both voices still hold the floor — only the identity evidence is missing"
    );
    let unembedded = voices
        .iter()
        .find(|cluster| !cell.embeddings.contains_key(cluster))
        .expect("one voice has no embedding");
    assert!(
        cell.turns.iter().any(|turn| turn.cluster == *unembedded),
        "the unembedded voice must still have its turns, or the scorer would \
         see no-turn where production sees no-embedding"
    );
}

/// The grid has to contain the point production runs, or none of it can be
/// The grid has to contain the point production runs, or none of it can be
/// read against what ships today.
#[test]
fn the_matcher_grid_holds_the_shipped_point() {
    assert!(
        GRID_FLOORS.contains(&diarize::cluster::MATCH_FLOOR),
        "shipped floor {} is not on the grid",
        diarize::cluster::MATCH_FLOOR
    );
    assert!(
        GRID_MARGINS.contains(&diarize::cluster::MATCH_MARGIN),
        "shipped margin {} is not on the grid",
        diarize::cluster::MATCH_MARGIN
    );
}

/// It exists because the first version of this scorer was wrong in a way no
/// It exists because the first version of this scorer was wrong in a way no
/// corpus run would have shown: it chose one representative cluster per
/// person and charged that person's whole speech time to it, so somebody 900
/// seconds right and 100 wrong came out as 1000 of one or the other. The
/// totals looked plausible either way. What catches that is an assertion that
/// seconds are conserved and that where a boundary falls cannot move them.
#[test]
fn recognition_seconds_are_conserved_and_unmoved_by_where_a_segment_splits() {
    let anchors: BTreeMap<String, Anchor> = [
        (
            "speaker-alice".to_string(),
            Anchor {
                who: "alice".into(),
                purity: 0.95,
                minted_in: "m1".into(),
            },
        ),
        (
            "speaker-bob".to_string(),
            Anchor {
                who: "bob".into(),
                purity: 0.90,
                minted_in: "m1".into(),
            },
        ),
        (
            "speaker-new".to_string(),
            Anchor {
                who: "carol".into(),
                purity: 0.99,
                minted_in: "m2".into(),
            },
        ),
    ]
    .into_iter()
    .collect();
    let enrolled: BTreeSet<String> = ["alice".to_string(), "bob".to_string()]
        .into_iter()
        .collect();
    let at = |who, stored| Scoring {
        population: Population::Returning,
        unscorable_returns: false,
        meeting: "m2",
        who,
        stored,
        anchors: &anchors,
        enrolled_before: &enrolled,
    };

    // Alice, returning, mostly found and partly confused for Bob, with some
    // of her speech never attributed at all — the shape the old scorer could
    // not represent.
    assert_eq!(
        classify(at("alice", Some("speaker-alice")), || Missing::NoTurn).tag(),
        "correct"
    );
    assert_eq!(
        classify(at("alice", Some("speaker-bob")), || Missing::NoTurn).tag(),
        "wrong"
    );
    assert_eq!(
        classify(at("alice", Some("speaker-new")), || Missing::NoTurn).tag(),
        "abstain-enrolled",
        "a Speaker minted in this very meeting is an abstention, not a mistake"
    );
    assert_eq!(
        classify(at("alice", None), || Missing::NoTurn).tag(),
        "unattributed:no-turn"
    );
    assert_eq!(
        classify(at("alice", None), || Missing::MintFloor).tag(),
        "unattributed:mint-floor"
    );

    // The two missing reasons come from production's signals, not duration.
    let cluster = diarize::Cluster(7);
    let long = diarize::Embedding::new(
        vec![1.0, 0.0],
        diarize::live::EMBEDDING_MODEL,
        diarize::live::EMBEDDING_MODEL_VERSION,
        diarize::cluster::MIN_SPEAKER_MS * 2,
    );
    let short = diarize::Embedding::new(
        vec![1.0, 0.0],
        diarize::live::EMBEDDING_MODEL,
        diarize::live::EMBEDDING_MODEL_VERSION,
        diarize::cluster::MIN_SPEAKER_MS / 2,
    );
    let with = |embedding: Option<diarize::Embedding>| diarize::Diarization {
        turns: Vec::new(),
        embeddings: embedding
            .map(|one| [(cluster, one)].into_iter().collect())
            .unwrap_or_default(),
    };
    let none: BTreeMap<diarize::Cluster, String> = BTreeMap::new();
    assert!(why_missing(None, &with(None), &none) == Missing::NoTurn);
    assert!(why_missing(Some(cluster), &with(None), &none) == Missing::NoEmbedding);
    assert!(why_missing(Some(cluster), &with(Some(short)), &none) == Missing::MintFloor);
    assert!(
        why_missing(Some(cluster), &with(Some(long.clone())), &none) == Missing::Unexpected,
        "over the floor and still unstored is not something persist does; say so"
    );
    let stored: BTreeMap<diarize::Cluster, String> = [(cluster, "speaker-alice".to_string())]
        .into_iter()
        .collect();
    assert!(
        why_missing(Some(cluster), &with(Some(long)), &stored) == Missing::Unexpected,
        "a cluster with an identity whose segment carries none is not a refusal"
    );

    // Seconds are conserved, and splitting a segment inside one outcome
    // cannot move them. The denominator is the reference, fixed.
    let coarse = vec![
        ("alice", "correct", 900.0),
        ("alice", "wrong", 100.0),
        ("bob", "unattributed:mint-floor", 50.0),
    ];
    let split = vec![
        ("alice", "correct", 400.0),
        ("alice", "correct", 500.0),
        ("alice", "wrong", 60.0),
        ("alice", "wrong", 40.0),
        ("bob", "unattributed:mint-floor", 20.0),
        ("bob", "unattributed:mint-floor", 30.0),
    ];
    let fold = |rows: &[(&str, &str, f64)]| -> BTreeMap<String, BTreeMap<String, f64>> {
        let mut out: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
        for (who, tag, seconds) in rows {
            *out.entry(who.to_string())
                .or_default()
                .entry(tag.to_string())
                .or_default() += seconds;
        }
        out
    };
    assert_eq!(fold(&coarse), fold(&split), "a split moved seconds");
    let total: f64 = coarse.iter().map(|(_, _, seconds)| seconds).sum();
    assert!(
        (total - 1050.0).abs() < 1e-9,
        "alice's 1000 seconds and bob's 50 are the reference denominator, \
         whatever the outcomes underneath"
    );
    assert_eq!(
        fold(&coarse)["alice"].values().sum::<f64>(),
        1000.0,
        "one person's seconds must not be charged wholly to one outcome"
    );
}

/// Replays a split, or pairs two replays that already ran.
///
/// ```text
/// EVERTRANSCRIPT_MEASURE_DER=1 EVERTRANSCRIPT_AMI_DIR=~/ami-dev \
/// EVERTRANSCRIPT_REPLAY_MANIFEST=<abs>/tests/ami-replay-dev.manifest \
/// EVERTRANSCRIPT_REPLAY_EVENTS=/tmp/dev-wespeaker.events \
/// EVERTRANSCRIPT_EMBEDDING=wespeaker EVERTRANSCRIPT_MERGE_SWEEP=0.65 \
///   cargo test --release -p evertranscript-core --test diarization_accuracy \
///   -- --nocapture the_gallery
/// ```
#[test]
fn the_gallery_recognizes_who_it_has_met_before() {
    if let Ok(pair) = std::env::var("EVERTRANSCRIPT_REPLAY_PAIR") {
        let (left, right) = pair.split_once(',').expect("two paths, comma separated");
        pair_replays(Path::new(left), Path::new(right));
        return;
    }

    let Ok(manifest) = std::env::var(REPLAY_ENV) else {
        eprintln!("{REPLAY_ENV} is unset — skipping the enrollment replay");
        return;
    };
    let Some(corpus) = corpus() else {
        // Silence here once cost a whole grid run: the manifest was set, the
        // corpus variable was not, and the test passed in no time at all
        // having replayed nothing.
        eprintln!("{MEASURE_ENV} is unset — skipping the enrollment replay");
        return;
    };
    let chapters = load_manifest(Path::new(&manifest), &corpus);

    let name = embedding_under_test().0.model;
    let (segmentation, embedding) = models();
    let thresholds = thresholds_under_test();
    assert_eq!(
        thresholds.len(),
        1,
        "the replay runs at one merge threshold; sweeping it would tune on the split under test"
    );
    let constrained = cannot_link_enabled();
    let grid = matcher_grid();
    println!(
        "replaying {} meetings through {name} at merge threshold {:.2}",
        chapters.len(),
        thresholds[0]
    );
    println!(
        "same-window cannot-link: {}",
        if constrained {
            "enforced"
        } else {
            "off (shipped clusterer)"
        }
    );
    println!(
        "matcher: {} configuration(s){}",
        grid.len(),
        if grid.len() == 1 {
            format!(" at floor {:.2} margin {:.2}", grid[0].0, grid[0].1)
        } else {
            String::from(", each with its own fresh store and gallery")
        }
    );
    println!("reference-transcript speaker-time: reference-derived segment boundaries, no ASR");
    println!("order within a-d is known; order across series and within IB is declared, not known");

    let started = Instant::now();

    // Inference once per meeting, then every configuration replays from the
    // same observations. Re-inferring per configuration would cost 32 passes
    // to vary two numbers that clustering never sees.
    let inferred: BTreeMap<String, Inferred> = chapters
        .iter()
        .map(|chapter| {
            let meeting = corpus
                .iter()
                .find(|one| one.name == chapter.meeting)
                .expect("manifest checked against the corpus already");
            let one = observe_once(meeting, &segmentation, &embedding);
            (chapter.meeting.clone(), one)
        })
        .collect();
    println!("inferred in {:.0}s\n", started.elapsed().as_secs_f64());

    let mut rows: Vec<(f32, f32, Event)> = Vec::new();
    let mut last: Option<(Vec<Event>, BTreeMap<String, Anchor>)> = None;
    for (floor, margin) in &grid {
        // A fresh store per configuration. Applying a new floor to a gallery
        // another floor already contaminated would measure neither.
        let (events, anchors) = replay(
            &chapters,
            &Cell::whole(&inferred, embedding.id),
            thresholds[0],
            constrained,
            *floor,
            *margin,
        );
        // The ledgers refuse a file that cannot be one, so running it here
        // means a malformed replay fails at the source, not at the pairing.
        let _ = ledgers(&events, &format!("floor {floor:.2} margin {margin:.2}"));
        if grid.len() > 1 {
            ledger_line(*floor, *margin, &events);
        }
        rows.extend(events.iter().cloned().map(|one| (*floor, *margin, one)));
        last = Some((events, anchors));
    }

    if grid.len() == 1 {
        let (events, anchors) = last.as_ref().expect("one configuration ran");
        report_replay(events, anchors);
    }
    println!("\nreplayed in {:.0}s", started.elapsed().as_secs_f64());

    if let Ok(path) = std::env::var("EVERTRANSCRIPT_REPLAY_EVENTS") {
        if grid.len() == 1 {
            write_events(Path::new(&path), &last.expect("one configuration ran").0);
        } else {
            write_grid_events(Path::new(&path), &rows);
        }
    }
}

/// The two models the split grid crosses, by the harness's own names.
const SPLIT_MODELS: [&str; 2] = ["wespeaker", "redimnet2"];

/// One clustering arm: whether segmentation's same-window cannot-link pairs
/// are enforced, and the merge threshold each model clusters at.
///
/// The threshold belongs to the model doing the clustering — it is where
/// *that* model's similarity distribution becomes a partition, and the two
/// distributions are not in the same place. Both were fixed by their own
/// sweeps already, so this grid does not sweep them again. The only thing it
/// varies is which model supplies the identity vectors.
struct SplitArm {
    label: &'static str,
    constrained: bool,
    /// Indexed by [`SPLIT_MODELS`], so a threshold cannot drift away from
    /// the model it was measured for.
    thresholds: [f32; SPLIT_MODELS.len()],
}

impl SplitArm {
    fn threshold(&self, model: &str) -> f32 {
        let at = SPLIT_MODELS
            .iter()
            .position(|&one| one == model)
            .expect("a model of the grid");
        self.thresholds[at]
    }
}

const SPLIT_ARMS: [SplitArm; 2] = [
    SplitArm {
        label: "unconstrained",
        constrained: false,
        thresholds: [0.65, 0.60],
    },
    SplitArm {
        label: "same-window cannot-link enforced",
        constrained: true,
        thresholds: [0.10, 0.10],
    },
];

/// Does clustering want the same embedding that identity wants?
///
/// Four cells per arm: each model's partition crossed with each model's
/// vectors. The diagonal is one model doing both jobs, which is what ships
/// and what every off-diagonal cell is read against.
///
/// ```text
/// EVERTRANSCRIPT_MEASURE_DER=1 EVERTRANSCRIPT_SPLIT_MODEL=1 \
/// EVERTRANSCRIPT_AMI_DIR=~/ami-dev \
/// EVERTRANSCRIPT_REPLAY_MANIFEST=<abs>/tests/ami-replay-dev.manifest \
/// EVERTRANSCRIPT_MATCHER_GRID=1 \
/// EVERTRANSCRIPT_REPLAY_EVENTS=/tmp/dev-split.events \
///   cargo test --release -p evertranscript-core --test diarization_accuracy \
///   -- --nocapture the_split
/// ```
#[test]
fn the_split_grid_changes_only_who_supplies_the_identity() {
    if std::env::var("EVERTRANSCRIPT_SPLIT_MODEL").as_deref() != Ok("1") {
        return;
    }

    let manifest = std::env::var(REPLAY_ENV).unwrap_or_else(|_| {
        panic!("EVERTRANSCRIPT_SPLIT_MODEL=1 needs {REPLAY_ENV}: the grid is a replay")
    });
    let corpus = corpus()
        .unwrap_or_else(|| panic!("EVERTRANSCRIPT_SPLIT_MODEL=1 needs {MEASURE_ENV} set too"));
    let chapters = load_manifest(Path::new(&manifest), &corpus);
    assert!(
        std::env::var_os("EVERTRANSCRIPT_MERGE_SWEEP").is_none(),
        "the split grid fixes each clustering model's own merge threshold; \
         sweeping them again would tune the partition on the split under test"
    );
    assert!(
        std::env::var_os("EVERTRANSCRIPT_CANNOT_LINK").is_none(),
        "the split grid runs both cannot-link arms itself"
    );

    let segmentation = model_directory().join("diarize-segmentation.onnx");
    assert!(
        segmentation.exists(),
        "the segmentation model is not at {}",
        segmentation.display()
    );
    let passes: BTreeMap<&str, Which> = SPLIT_MODELS
        .iter()
        .map(|&name| (name, which(name)))
        .collect();

    println!(
        "split-model grid over {} meetings: the clustering model's partition, \
         the identity model's vectors",
        chapters.len()
    );
    for name in SPLIT_MODELS {
        let pass = &passes[name];
        println!(
            "  {name}: {} v{} ({})",
            pass.id.model,
            pass.id.version,
            pass.path.display()
        );
    }
    println!("segmentation step: {} ms", step_under_test());
    println!("reference-transcript speaker-time: reference-derived segment boundaries, no ASR");
    println!("order within a-d is known; order across series and within IB is declared, not known");
    println!(
        "\nreading rule, declared before any of these numbers were read. The four \
         ledger quantities are returning correct, returning wrong, new correct and \
         newcomer false attachment. One cell DOMINATES another when it is at least \
         as good on all four and strictly better on one; equal on all four is a TIE; \
         anything else is a TRADE, and no rate of exchange between the four is \
         assumed. An off-diagonal cell dominating both same-model controls of its arm \
         is SUFFICIENT to establish a recognition benefit at that configuration. It is \
         NOT necessary for a split to be worth having: these four are the recognition \
         column only, and say nothing about the DER column, about inference cost, or \
         about the second model and the second inference pass (a split does not entail two persisted identity spaces per Speaker: clustering vectors can stay meeting-local). A cell that is a trade here \
         may still be worth having, and a cell that dominates here may not be. So what \
         follows reports ties, dominances and trades against the configurations they \
         hold at, and stops there: any recommendation is a separate statement, made \
         separately, and the adoption bar is the user's and is undecided."
    );

    // Inference once per meeting per embedding, reused by every cell and
    // every matcher point: the vectors do not depend on the threshold, the
    // constraint, or which job the model is being asked to do here.
    let inferred: BTreeMap<&str, BTreeMap<String, Inferred>> = SPLIT_MODELS
        .iter()
        .map(|&name| {
            let started = Instant::now();
            let one: BTreeMap<String, Inferred> = chapters
                .iter()
                .map(|chapter| {
                    let meeting = corpus
                        .iter()
                        .find(|one| one.name == chapter.meeting)
                        .expect("manifest checked against the corpus already");
                    (
                        chapter.meeting.clone(),
                        observe_once(meeting, &segmentation, &passes[name]),
                    )
                })
                .collect();
            println!(
                "\ninferred {name} over {} meetings in {:.0}s",
                one.len(),
                started.elapsed().as_secs_f64()
            );
            (name, one)
        })
        .collect();

    // Coverage, per pass, before anything is scored.
    //
    // The two passes can disagree about which observations exist, and the
    // disagreement is not symmetric. Its denominator is one pass's own
    // observations: adding the two passes together would count every shared
    // track twice and report a smaller share than the one that matters.
    for &clustering in &SPLIT_MODELS {
        for &identity in &SPLIT_MODELS {
            if clustering == identity {
                continue;
            }
            let mut support = Support::default();
            for chapter in &chapters {
                let (_, one) = wearing(
                    &inferred[clustering][&chapter.meeting].observed,
                    &inferred[identity][&chapter.meeting].observed,
                    &chapter.meeting,
                );
                support.add(one);
            }
            println!(
                "  {identity} covers {:.3}% of {clustering}'s {} observations: {} with a \
                 vector, {} without ({:.1}s voiced, which keep their turns and their \
                 cluster and lend nothing to its centroid). {} of {identity}'s own \
                 observations have no place in {clustering}'s partition and are \
                 excluded ({:.1}s).",
                support.coverage() * 100.0,
                support.matched + support.unvectored,
                support.matched,
                support.unvectored,
                support.unvectored_ms as f64 / 1000.0,
                support.unplaced,
                support.unplaced_ms as f64 / 1000.0,
            );
        }
    }

    let grid = matcher_grid();
    for arm in &SPLIT_ARMS {
        println!("\n=== {} ===", arm.label);

        // Turn placement first, because it is the premise of the whole grid:
        // a split cell may not move DER at all. Identity vectors are not
        // supposed to reach turn placement, so a difference here would be
        // the splice leaking into the partition rather than anything about
        // the models. It holds under thin identity support too: the
        // partition comes from the clustering pass's own complete vectors.
        for &clustering in &SPLIT_MODELS {
            let threshold = arm.threshold(clustering);
            let tally_for = |identity: &str| {
                let mut tally = score::Der::default();
                for chapter in &chapters {
                    let partition = &inferred[clustering][&chapter.meeting];
                    let vectors = &inferred[identity][&chapter.meeting];
                    let (diarization, _) = split_clustered(
                        &partition.observed,
                        &vectors.observed,
                        &chapter.meeting,
                        threshold,
                        arm.constrained,
                    );
                    tally.accumulate(&score::der(
                        &partition.reference,
                        &hypothesis(&diarization.turns),
                    ));
                }
                tally
            };
            // The control first, whatever order the models are listed in:
            // it is what every other cell with this partition is read
            // against, so it cannot be computed halfway through the loop.
            let control = tally_for(clustering);
            let four = |der: &score::Der| {
                (
                    der.total_ms,
                    der.missed_ms,
                    der.false_alarm_ms,
                    der.confusion_ms,
                )
            };
            for &identity in &SPLIT_MODELS {
                if identity == clustering {
                    continue;
                }
                assert_eq!(
                    four(&tally_for(identity)),
                    four(&control),
                    "{clustering} clustering / {identity} identity moved DER away \
                     from its control. The partition and the turns are the \
                     clustering pass's in both, so the identity vectors have \
                     reached turn placement — a rig fault, not a model difference."
                );
            }
            println!(
                "  {clustering} clustering at merge {threshold:.2}: DER {:.2}% \
                 (missed {:.2} false alarm {:.2} confusion {:.2}) — every cell \
                 with this partition scores it",
                control.rate() * 100.0,
                control.missed_rate() * 100.0,
                control.false_alarm_rate() * 100.0,
                control.confusion_rate() * 100.0,
            );
        }

        for &clustering in &SPLIT_MODELS {
            for &identity in &SPLIT_MODELS {
                let cell = Cell {
                    clustering: &inferred[clustering],
                    identity: &inferred[identity],
                    stamp: passes[identity].id,
                };
                println!(
                    "\n  {clustering} clustering / {identity} identity{}",
                    if clustering == identity {
                        "   (same-model control)"
                    } else {
                        ""
                    }
                );
                let mut rows: Vec<(f32, f32, Event)> = Vec::new();
                for &(floor, margin) in &grid {
                    let (events, _) = replay(
                        &chapters,
                        &cell,
                        arm.threshold(clustering),
                        arm.constrained,
                        floor,
                        margin,
                    );
                    // The ledgers refuse a file that cannot be one, so a
                    // malformed cell fails here rather than at the pairing.
                    let _ = ledgers(
                        &events,
                        &format!("{clustering}/{identity} floor {floor:.2} margin {margin:.2}"),
                    );
                    ledger_line(floor, margin, &events);
                    rows.extend(events.into_iter().map(|one| (floor, margin, one)));
                }
                if let Ok(path) = std::env::var("EVERTRANSCRIPT_REPLAY_EVENTS") {
                    let arm_slug = if arm.constrained {
                        "constrained"
                    } else {
                        "unconstrained"
                    };
                    write_grid_events(
                        Path::new(&format!("{path}.{arm_slug}.{clustering}-{identity}")),
                        &rows,
                    );
                }
            }
        }
    }
}
