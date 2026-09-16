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
use std::path::Path;
use std::path::PathBuf;
use std::time::Instant;

use evertranscript_core::diarize;
use evertranscript_core::diarize::score;
use evertranscript_core::diarize::score::Der;
use evertranscript_core::diarize::score::Span;
use evertranscript_core::diarize::score::Trial;

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
///   EVERTRANSCRIPT_EMBEDDING=wespeaker   (default) diarize-embedding.onnx
///   EVERTRANSCRIPT_EMBEDDING=redimnet2             diarize-embedding-redimnet2.onnx
fn embedding_under_test() -> (String, PathBuf, diarize::live::Frontend) {
    match std::env::var("EVERTRANSCRIPT_EMBEDDING")
        .unwrap_or_else(|_| "wespeaker".into())
        .as_str()
    {
        "redimnet2" => (
            "redimnet2-b3".into(),
            "diarize-embedding-redimnet2.onnx".into(),
            diarize::live::Frontend::Waveform,
        ),
        "wespeaker" => (
            "wespeaker-resnet34-LM".into(),
            "diarize-embedding.onnx".into(),
            diarize::live::Frontend::Fbank,
        ),
        other => panic!("EVERTRANSCRIPT_EMBEDDING={other}: expected wespeaker or redimnet2"),
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

fn models() -> (PathBuf, PathBuf) {
    let dir = std::env::var_os("EVERTRANSCRIPT_MODELS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(evertranscript_core::paths::models_dir);
    let segmentation = dir.join("diarize-segmentation.onnx");
    let (name, file, _) = embedding_under_test();
    let embedding = dir.join(&file);
    println!("embedding under test: {name} ({})", file.display());
    println!("segmentation step: {} ms", step_under_test());
    assert!(
        segmentation.exists() && embedding.exists(),
        "the diarization models are not in {}. Fetch them with \
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
    cannot_link: (u64, u64),
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

/// Run the models over one meeting.
fn observe_once(meeting: &Meeting, segmentation: &Path, embedding: &Path) -> Inferred {
    let samples = read_wav(&meeting.audio);
    let audio_seconds = samples.len() as f64 / diarize::fbank::SAMPLE_RATE as f64;
    let reference =
        score::parse_rttm(&std::fs::read_to_string(&meeting.reference).expect("read reference"));

    let mut diarizer =
        diarize::live::LiveDiarizer::load_with(segmentation, embedding, embedding_under_test().2)
            .expect("load models")
            .with_step(step_under_test());

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

    Inferred {
        name: meeting.name.clone(),
        reference,
        observed,
        audio_seconds,
        seconds,
    }
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
fn same_window_merges(one: &Inferred, threshold: f32) -> (u64, u64) {
    let provisional = diarize::live::provisional_of(&one.observed);
    let canonical = diarize::cluster::agglomerate_with(&provisional, threshold);

    // Observation indices grouped by the window they were heard in, each
    // carrying who the reference says it actually is.
    let mut per_window: BTreeMap<usize, Vec<(diarize::Cluster, String)>> = BTreeMap::new();
    for observation in &one.observed.observations {
        let Some((start, _)) = observation.runs.first() else {
            continue;
        };
        let Some(window) = one
            .observed
            .windows
            .iter()
            .position(|&(channel, from, to)| {
                channel == observation.channel && *start >= from && *start < to
            })
        else {
            continue;
        };
        // No owner in the reference means the pair says nothing either way.
        if let Some(who) = dominant_speaker(&observation.runs, &one.reference) {
            per_window
                .entry(window)
                .or_default()
                .push((observation.cluster, who));
        }
    }

    let (mut merged, mut pairs) = (0u64, 0u64);
    for held in per_window.values() {
        for (i, (left, left_who)) in held.iter().enumerate() {
            for (right, right_who) in &held[i + 1..] {
                if left_who == right_who {
                    continue;
                }
                pairs += 1;
                if canonical.get(left) == canonical.get(right) {
                    merged += 1;
                }
            }
        }
    }
    (merged, pairs)
}

fn score_at(one: &Inferred, threshold: f32, with_windows: bool) -> Measured {
    let reference = &one.reference;
    let result = diarize::live::cluster_observed(&one.observed, threshold);
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
        "same-window    {:>6.2}%   {merged} of {pairs} pairs heard talking at once were merged anyway",
        if pairs == 0 {
            0.0
        } else {
            merged as f64 / pairs as f64 * 100.0
        }
    );
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
            "\noracle voices  {} voices from perfect clustering — the ceiling any \
             threshold aims at",
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
