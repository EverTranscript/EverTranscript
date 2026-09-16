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
/// Where to keep each meeting's voiceprints between runs.
///
/// **Cross-meeting EER needs every meeting's voices in one place, and a
/// process holding all sixteen is the one thing this machine could not
/// do** — the corpus run was killed three times by a low-memory guard and
/// had to be taken a meeting per process. Writing each meeting's voiceprints
/// here as it is measured makes the pooled EER a second pass over the cache
/// rather than a property of one long-lived process, so a corpus measured in
/// sixteen runs scores the same as one measured in a single run.
///
/// Unset means no cache and EER only across the meetings this process saw,
/// which is the old behaviour and right for a single-run corpus.
const VOICEPRINT_CACHE_ENV: &str = "EVERTRANSCRIPT_VOICEPRINT_CACHE";
/// Skip the DER pass and derive thresholds from the cache alone.
const ANALYZE_ONLY_ENV: &str = "EVERTRANSCRIPT_ANALYZE_ONLY";

/// The floor a Voiceprint has to clear to be a match, as `diarize::cluster`
/// ships it. Duplicated rather than imported because the point of reporting
/// a rate at this number is to notice when the constant and the model have
/// drifted apart — an import would silently follow one of them.
const MATCH_FLOOR: f32 = 0.62;

/// The merge threshold as `diarize::cluster` ships it, duplicated for the
/// same reason as [`MATCH_FLOOR`].
const MERGE_THRESHOLD: f32 = 0.6;

/// The margin as `diarize::cluster` ships it, duplicated for the same reason
/// as [`MATCH_FLOOR`].
const MATCH_MARGIN: f32 = 0.08;

/// Whether to keep every pre-merge window vector as well as the per-voice
/// ones.
///
/// Off by default because it is the expensive half: a meeting is thousands
/// of windows where it is a handful of voices. It earns the cost once —
/// the merge threshold decides which windows are the same person, so it
/// cannot be chosen from vectors that a merge has already been applied to,
/// and with these cached every candidate threshold is scored from one pass
/// over the audio rather than one pass each.
const KEEP_WINDOWS_ENV: &str = "EVERTRANSCRIPT_KEEP_WINDOWS";

/// How many of a meeting's windows the merge curve pairs up.
///
/// Pairs grow as the square, so a meeting's full fourteen hundred windows
/// are a million pairs and the corpus is tens of millions — which is the
/// memory this whole measurement is arranged to avoid. Four hundred is
/// eighty thousand pairs a meeting, a few million over the corpus, and far
/// more than a threshold curve needs to be steady.
const WINDOW_SAMPLE: usize = 400;

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
fn models() -> (PathBuf, PathBuf) {
    let dir = std::env::var_os("EVERTRANSCRIPT_MODELS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(evertranscript_core::paths::models_dir);
    let segmentation = dir.join("diarize-segmentation.onnx");
    let embedding = dir.join("diarize-embedding.onnx");
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
        diarize::SAMPLE_RATE,
        "{} is at {} Hz; scripts/fetch-ami.sh resamples to {}",
        path.display(),
        spec.sample_rate,
        diarize::SAMPLE_RATE
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
    /// Pre-merge window vectors, labelled by reference speaker. Empty unless
    /// [`KEEP_WINDOWS_ENV`] asked for them.
    windows: Vec<(String, Vec<f32>)>,
}

/// The reference speaker holding most of these milliseconds, if any is.
///
/// A window straddling a speaker change belongs to whoever is in most of it;
/// one that is all silence or all overlap belongs to nobody and is dropped,
/// because a vector with no owner cannot say whether a merge was right.
fn dominant_speaker(ranges: &[(u64, u64)], reference: &[score::Span]) -> Option<String> {
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

fn measure(meeting: &Meeting, segmentation: &Path, embedding: &Path) -> Measured {
    let samples = read_wav(&meeting.audio);
    let audio_seconds = samples.len() as f64 / diarize::SAMPLE_RATE as f64;
    let reference =
        score::parse_rttm(&std::fs::read_to_string(&meeting.reference).expect("read reference"));

    let mut diarizer =
        diarize::live::LiveDiarizer::load(segmentation, embedding).expect("load models");
    let keep_windows = std::env::var_os(KEEP_WINDOWS_ENV).is_some();
    diarizer.keep_windows(keep_windows);
    let audio = diarize::MeetingAudio {
        mic: &samples,
        system: &[],
        sample_rate: diarize::SAMPLE_RATE,
    };

    // Wall clock per meeting, because a ceiling is one of the things being
    // fixed: clustering was cubic, and 70 s at 1,259 windows projected to a
    // quarter of an hour for a two-hour meeting.
    let started = Instant::now();
    let result =
        diarize::runner::run_guarded(&mut diarizer, audio, &mut |_| {}, &diarize::Cancel::new())
            .expect("diarize");
    let seconds = started.elapsed().as_secs_f64();

    // Labelled by the reference speaker each window mostly belongs to, which
    // is what turns a pile of vectors into evidence about a merge threshold:
    // two windows of one person *should* merge, two of different people
    // should not, and a threshold is a guess at where that line is.
    let windows: Vec<(String, Vec<f32>)> = diarizer
        .windows()
        .iter()
        .filter_map(|window| {
            let who = dominant_speaker(&window.ranges, &reference)?;
            Some((who, window.vector.clone()))
        })
        .collect();

    let spans = hypothesis(&result.turns);
    Measured {
        windows,
        name: meeting.name.clone(),
        der: score::der(&reference, &spans),
        oracle: score::der(&reference, &score::oracle_relabel(&spans, &reference)),
        seconds,
        audio_seconds,
        // Labelled by the reference speaker the cluster mostly is, so a
        // cross-meeting trial knows whether two vectors are the same person.
        embeddings: result
            .embeddings
            .iter()
            .filter_map(|(cluster, embedding)| {
                let own: Vec<Span> = spans
                    .iter()
                    .filter(|span| span.speaker == format!("cluster-{}", cluster.index()))
                    .cloned()
                    .collect();
                let named = score::oracle_relabel(&own, &reference);
                let who = named.first()?.speaker.clone();
                (!who.starts_with("cluster-")).then(|| (who, embedding.vector.clone()))
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

/// Writes one meeting's voiceprints to the cache, if there is one.
///
/// One line per voice: the reference speaker it belongs to, a tab, then the
/// vector. Text because the file is evidence a person may want to look at,
/// and the whole corpus is a few hundred kilobytes.
fn cache_voiceprints(measured: &Measured) {
    let Some(dir) = std::env::var_os(VOICEPRINT_CACHE_ENV) else {
        return;
    };
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir).expect("create voiceprint cache");
    let lines = |rows: &[(String, Vec<f32>)]| -> String {
        rows.iter()
            .map(|(speaker, vector)| {
                let numbers: Vec<String> = vector.iter().map(|v| format!("{v}")).collect();
                format!("{speaker}\t{}\n", numbers.join(","))
            })
            .collect()
    };
    // Written whole rather than appended, so re-measuring a meeting replaces
    // its voices instead of adding a second copy that would then be paired
    // against the first as though it were another meeting.
    std::fs::write(
        dir.join(format!("{}.voiceprints", measured.name)),
        lines(&measured.embeddings),
    )
    .expect("write voiceprints");
    // Same format, separate file: one is a meeting's voices, the other its
    // windows, and the two answer different questions. Written only when
    // there are some, so a cache built without them stays as it was.
    if !measured.windows.is_empty() {
        std::fs::write(
            dir.join(format!("{}.windows", measured.name)),
            lines(&measured.windows),
        )
        .expect("write windows");
    }
}

/// One meeting-worth of labelled vectors: who each is, and the vector.
type Labelled = Vec<(String, Vec<f32>)>;

/// Every cached meeting's pre-merge windows, as `(meeting, its windows)`.
fn cached_windows() -> Vec<(String, Labelled)> {
    let Some(dir) = std::env::var_os(VOICEPRINT_CACHE_ENV) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(PathBuf::from(dir)) else {
        return Vec::new();
    };
    let mut meetings: Vec<(String, Labelled)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension()? != "windows" {
                return None;
            }
            let rows = parse_vectors(&std::fs::read_to_string(&path).ok()?);
            let name = path.file_stem()?.to_string_lossy().to_string();
            (!rows.is_empty()).then_some((name, rows))
        })
        .collect();
    meetings.sort_by(|a, b| a.0.cmp(&b.0));
    meetings
}

/// The voices one meeting's windows would become if they were agglomerated
/// at `threshold`, each labelled by the reference speaker it mostly is.
///
/// **This is the step that makes a merge threshold answerable.** A merge
/// threshold's real cost is not paid inside the meeting — it is paid when
/// the voices it produced are matched against another meeting's. Too high a
/// threshold splits one person into fragments, and a fragment is a worse
/// Voiceprint than a whole voice: it carries less of the speaker and more of
/// whatever they happened to be saying. So the threshold has to be scored on
/// what it does to recognition, and that means rebuilding the voices at each
/// candidate rather than measuring the ones 0.60 happened to make.
fn voices_at(windows: &Labelled, threshold: f32) -> (Labelled, Quality) {
    let (voices, quality) = voices_at_with_sizes(windows, threshold, 0);
    (voices, quality)
}

/// [`voices_at`], dropping any group holding fewer than `floor` windows.
fn voices_at_with_sizes(windows: &Labelled, threshold: f32, floor: usize) -> (Labelled, Quality) {
    let provisional: BTreeMap<diarize::Cluster, diarize::Embedding> = windows
        .iter()
        .enumerate()
        .map(|(index, (_, vector))| {
            (
                diarize::Cluster(index as u32),
                diarize::Embedding::new(vector.clone(), "cache", "1", 1),
            )
        })
        .collect();
    let canonical = diarize::cluster::agglomerate_at(&provisional, threshold);

    let mut members: BTreeMap<diarize::Cluster, Vec<usize>> = BTreeMap::new();
    for (index, _) in windows.iter().enumerate() {
        let cluster = diarize::Cluster(index as u32);
        let group = canonical.get(&cluster).copied().unwrap_or(cluster);
        members.entry(group).or_default().push(index);
    }

    let (voices, agreed): (Labelled, Vec<usize>) = members
        .into_values()
        .filter_map(|group| {
            // Whoever holds most of the group's windows. A group that is
            // mostly one person and partly another is that person's, which
            // is what production would do with it too.
            let mut votes: BTreeMap<&str, usize> = BTreeMap::new();
            for index in &group {
                *votes.entry(windows[*index].0.as_str()).or_default() += 1;
            }
            if group.len() < floor {
                return None;
            }
            let (who, best) = votes.into_iter().max_by_key(|(_, count)| *count)?;
            let who = who.to_string();

            let width = windows[group[0]].1.len();
            let mut centre = vec![0.0f32; width];
            for index in &group {
                for (slot, value) in centre.iter_mut().zip(&windows[*index].1) {
                    *slot += value;
                }
            }
            let norm: f32 = centre.iter().map(|v| v * v).sum::<f32>().sqrt();
            if norm > 0.0 {
                for slot in centre.iter_mut() {
                    *slot /= norm;
                }
            }
            Some(((who, centre), best))
        })
        .unzip();
    let speakers: std::collections::BTreeSet<&str> =
        windows.iter().map(|(who, _)| who.as_str()).collect();
    (
        voices,
        Quality {
            purity: agreed.iter().sum::<usize>() as f64 / windows.len().max(1) as f64,
            splinter: agreed.len() as f64 / speakers.len().max(1) as f64,
        },
    )
}

/// What a merge threshold did to the *partition*, as opposed to what it did
/// to recognition.
///
/// **The recognition sweep cannot choose a merge threshold on its own, and
/// reporting it alone would be misleading.** Recognition improves
/// monotonically as merging stops, because an unmerged window is a pure
/// window; taken by itself that argues for never merging, which would hand
/// the Operator forty "Speaker N"s in a four-person meeting. These two
/// numbers are the other side of that trade. A DER would fold both into one,
/// but a DER needs the windows' timestamps and those are not in the cache.
struct Quality {
    /// Share of windows sitting with their own majority — the contamination
    /// a low threshold causes.
    purity: f64,
    /// Groups per real speaker — the fragmentation a high one causes.
    splinter: f64,
}

/// `speaker<TAB>comma,separated,vector` per line.
fn parse_vectors(text: &str) -> Labelled {
    text.lines()
        .filter_map(|line| {
            let (speaker, vector) = line.split_once('\t')?;
            let vector: Vec<f32> = vector.split(',').filter_map(|v| v.parse().ok()).collect();
            (!vector.is_empty()).then(|| (speaker.to_string(), vector))
        })
        .collect()
}

/// Every meeting in the cache, as the trial builder wants them.
///
/// Returns `None` when there is no cache, which leaves the caller on the
/// meetings this process measured.
fn cached_voiceprints() -> Option<Vec<Measured>> {
    let dir = PathBuf::from(std::env::var_os(VOICEPRINT_CACHE_ENV)?);
    let mut meetings: Vec<Measured> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension()? != "voiceprints" {
                return None;
            }
            let embeddings = parse_vectors(&std::fs::read_to_string(&path).ok()?);
            Some(Measured {
                name: path.file_stem()?.to_string_lossy().to_string(),
                der: Der::default(),
                oracle: Der::default(),
                seconds: 0.0,
                audio_seconds: 0.0,
                embeddings,
                windows: Vec::new(),
            })
        })
        .collect();
    meetings.sort_by(|a, b| a.name.cmp(&b.name));
    (!meetings.is_empty()).then_some(meetings)
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
    let meetings: Vec<(String, Labelled)> = measured
        .iter()
        .map(|one| (one.name.clone(), one.embeddings.clone()))
        .collect();
    cross_meeting_pairs(&meetings)
}

/// The same pairing, over voices that did not come from a `Measured` — the
/// ones rebuilt by [`voices_at`] at a candidate merge threshold. One rule
/// rather than two, so a sweep cannot quietly score itself more kindly than
/// the headline figure does.
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
    // Re-deriving a threshold reads the cache and touches no model, so the
    // DER pass in front of it is two hours of inference for numbers that are
    // already on disk. This is the switch that separates the two jobs: with
    // it set, the run is an analysis of what was measured before.
    let analyze_only = std::env::var_os(ANALYZE_ONLY_ENV).is_some();
    if analyze_only && cached_voiceprints().is_none() {
        println!(
            "{ANALYZE_ONLY_ENV} is set but the voiceprint cache is empty — nothing to analyze"
        );
        return;
    }
    let (segmentation, embedding) = if analyze_only {
        println!("analysis only: DER not re-measured, thresholds derived from the cache");
        (PathBuf::new(), PathBuf::new())
    } else {
        models()
    };

    let measured: Vec<Measured> = meetings
        .iter()
        .filter(|_| !analyze_only)
        .map(|meeting| {
            let one = measure(meeting, &segmentation, &embedding);
            // Per meeting, so one bad meeting is visible rather than
            // averaged into the corpus figure.
            // `ref` is the denominator, printed so a run that dies part way
            // through is still poolable from its own output: the pooled
            // figure weighs each meeting by its reference speech, and
            // without it the per-meeting rates cannot be combined.
            println!(
                "{:<12}  DER {:>6.1}%  (missed {:>5.1}  false alarm {:>5.1}  confusion {:>5.1})  \
                 oracle {:>6.1}%  ref {:>7.1}s  {:>6.1}s for {:>6.1}s of audio  ({:.2}x)",
                one.name,
                one.der.rate() * 100.0,
                one.der.missed_rate() * 100.0,
                one.der.false_alarm_rate() * 100.0,
                one.der.confusion_rate() * 100.0,
                one.oracle.rate() * 100.0,
                one.der.total_ms as f64 / 1000.0,
                one.seconds,
                one.audio_seconds,
                one.seconds / one.audio_seconds.max(1.0),
            );
            cache_voiceprints(&one);
            one
        })
        .collect();

    // Pooled, not averaged: a ninety-second meeting must not weigh as much
    // as a fifty-minute one, and pyannote's published figure is pooled.
    let mut pooled = Der::default();
    let mut oracle = Der::default();
    for one in &measured {
        pooled.accumulate(&one.der);
        oracle.accumulate(&one.oracle);
    }

    // Over the cache when there is one, so a corpus measured a meeting per
    // process scores the same EER as a corpus measured in one run.
    let for_eer = cached_voiceprints().unwrap_or_else(|| {
        measured
            .iter()
            .map(|one| Measured {
                name: one.name.clone(),
                der: Der::default(),
                oracle: Der::default(),
                seconds: 0.0,
                audio_seconds: 0.0,
                embeddings: one.embeddings.clone(),
                windows: Vec::new(),
            })
            .collect()
    });
    let trials = cross_meeting_trials(&for_eer);
    let eer = score::equal_error_rate(&trials);
    let false_accepts = score::false_accept_rate_at(&trials, MATCH_FLOOR);
    if for_eer.len() != measured.len() {
        println!(
            "\nEER pooled over {} cached meetings ({} measured this run)",
            for_eer.len(),
            measured.len()
        );
    }

    println!("\n{} meetings", measured.len());
    // Silent rather than zero when nothing was measured: a DER of 0.00%
    // printed by a run that did not measure one is the most misleading line
    // this harness could emit.
    if measured.is_empty() {
        println!("DER            not measured this run");
    } else {
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
    }
    match eer {
        Some((rate, threshold)) => println!(
            "cross-meeting  EER {:>5.2}% at {threshold:.3}   {} trials",
            rate * 100.0,
            trials.len()
        ),
        None => println!("cross-meeting  EER not computable: one class is empty"),
    }
    if let Some(rate) = false_accepts {
        println!(
            "at MATCH_FLOOR {MATCH_FLOOR}: {:>5.1}% of different colleagues would be \
             accepted as the same person",
            rate * 100.0
        );
    }
    // Beside EER, not instead of it. EER asks whether a pair would pass a
    // bar; this asks whether the right person won, which is the question an
    // Operator feels. A model can have a respectable EER and still put the
    // wrong colleague first, because the pairs it gets wrong are exactly the
    // ones that compete.
    let candidates: Vec<score::Candidate> = for_eer
        .iter()
        .flat_map(|one| {
            one.embeddings
                .iter()
                .map(|(speaker, vector)| score::Candidate {
                    speaker,
                    group: &one.name,
                    vector,
                })
        })
        .collect();
    match score::nearest_is_right(&candidates) {
        Some(rate) => println!(
            "nearest voice  {:>5.1}% right   {} voices, each against every other meeting's",
            rate * 100.0,
            candidates.len()
        ),
        None => println!("nearest voice  not askable: one meeting's voices have nobody to meet"),
    }

    // The curve, not the point. A threshold reported as one number says what
    // was chosen and nothing about how much the choice was worth — and the
    // M3 close-out's dev curve was flat from 0.40 to 0.60 while test moved
    // eight points over the same range, which a single number would have
    // hidden completely.
    let sweep: Vec<f32> = (30..=90)
        .step_by(5)
        .map(|step| step as f32 / 100.0)
        .collect();
    match score::refusal_point(&trials) {
        Some((threshold, refused)) => println!(
            "floor that admits nobody: {threshold:.3}, refusing {:.2}% of genuine pairs",
            refused * 100.0
        ),
        None => println!("floor that admits nobody: not askable, no impostor pairs"),
    }
    println!("\nthreshold   false accept   false reject");
    for point in score::curve(&trials, &sweep) {
        println!(
            "  {:.2}        {:>6.2}%        {:>6.2}%{}",
            point.threshold,
            point.false_accept * 100.0,
            point.false_reject * 100.0,
            if (point.threshold - MATCH_FLOOR).abs() < 1e-6 {
                "   <- MATCH_FLOOR"
            } else {
                ""
            }
        );
    }
    // The margin, which is neither the floor nor the merge threshold. The
    // floor asks whether a score is high enough to be anybody; the margin
    // asks whether the winner beat the runner-up widely enough to be sure it
    // is *this* person. Its trials are gaps, so the numbers below are on a
    // different scale from the two curves either side of them.
    let margins = score::margin_trials(&candidates);
    match score::equal_error_rate(&margins) {
        Some((rate, threshold)) => println!(
            "\nmargin         EER {:>5.2}% at {threshold:.3}   {} probes",
            rate * 100.0,
            margins.len()
        ),
        None => println!("\nmargin  not computable: every winner was right, or none was"),
    }
    match score::refusal_point(&margins) {
        Some((threshold, refused)) => println!(
            "margin that admits no wrong winner: {threshold:.3}, refusing {:.2}% of right ones",
            refused * 100.0
        ),
        None => println!("margin that admits no wrong winner: not askable, every winner was right"),
    }
    let margin_sweep: Vec<f32> = (0..=20).map(|step| step as f32 / 100.0).collect();
    println!("margin      wrong winner accepted   right winner refused");
    for point in score::curve(&margins, &margin_sweep) {
        println!(
            "  {:.2}        {:>6.2}%                {:>6.2}%{}",
            point.threshold,
            point.false_accept * 100.0,
            point.false_reject * 100.0,
            if (point.threshold - MATCH_MARGIN).abs() < 1e-6 {
                "   <- MATCH_MARGIN"
            } else {
                ""
            }
        );
    }

    // The merge threshold, which is a different question from the floor and
    // needs different evidence. The floor compares one meeting's voice to
    // another's; the merge threshold compares two windows *inside* one
    // meeting, where the pair shares a room, a microphone and a minute of
    // acoustics. Scoring it on cross-meeting pairs would choose it against
    // the wrong distribution entirely.
    let windows = cached_windows();
    if windows.is_empty() {
        println!(
            "\nmerge threshold  no window cache — re-run with {KEEP_WINDOWS_ENV}=1 to derive it"
        );
    } else {
        // Every window against every other *in its own meeting* would be a
        // million pairs a meeting and tens of millions over the corpus —
        // hundreds of megabytes on the machine whose memory guard is the
        // reason this measurement is taken a meeting per process at all. An
        // evenly spaced sample is plenty for a curve, and spacing it rather
        // than taking a prefix matters: the first four hundred windows of a
        // meeting are its first few minutes, when half the room has not
        // spoken yet.
        let sampled: Vec<(&String, Labelled)> = windows
            .iter()
            .map(|(name, rows)| {
                let step = rows.len().div_ceil(WINDOW_SAMPLE).max(1);
                (name, rows.iter().step_by(step).cloned().collect())
            })
            .collect();
        let within: Vec<score::Trial> = sampled
            .iter()
            .flat_map(|(_, rows)| {
                rows.iter().enumerate().flat_map(move |(index, left)| {
                    rows[index + 1..].iter().map(move |right| score::Trial {
                        score: cosine(&left.1, &right.1),
                        same_speaker: left.0 == right.0,
                    })
                })
            })
            .collect();
        let voices: usize = sampled.iter().map(|(_, rows)| rows.len()).sum();
        match score::equal_error_rate(&within) {
            Some((rate, threshold)) => println!(
                "\nwithin-meeting EER {:>5.2}% at {threshold:.3}   \
                 {voices} windows over {} meetings, {} pairs",
                rate * 100.0,
                windows.len(),
                within.len()
            ),
            None => println!("\nwithin-meeting EER not computable: one class is empty"),
        }
        println!("threshold   false accept   false reject");
        for point in score::curve(&within, &sweep) {
            println!(
                "  {:.2}        {:>6.2}%        {:>6.2}%{}",
                point.threshold,
                point.false_accept * 100.0,
                point.false_reject * 100.0,
                if (point.threshold - MERGE_THRESHOLD).abs() < 1e-6 {
                    "   <- MERGE_THRESHOLD"
                } else {
                    ""
                }
            );
        }

        // The ceiling, before any threshold is chosen. These voices are
        // built by the reference: every window a speaker actually owns,
        // summed into one centroid, with no clustering in the way. It is the
        // best any merge threshold could do, and it is the number that says
        // where a miss lives. If the pipeline's EER is far above this, the
        // clustering is at fault and a better threshold is worth looking
        // for; if this is itself far above the bar, no threshold reaches it
        // and the embedding or the audio is the thing to change.
        let oracle: Vec<(String, Labelled)> = windows
            .iter()
            .map(|(name, rows)| {
                let mut sums: BTreeMap<&str, Vec<f32>> = BTreeMap::new();
                for (who, vector) in rows {
                    let centre = sums
                        .entry(who.as_str())
                        .or_insert_with(|| vec![0.0; vector.len()]);
                    for (slot, value) in centre.iter_mut().zip(vector) {
                        *slot += value;
                    }
                }
                let voices = sums
                    .into_iter()
                    .map(|(who, mut centre)| {
                        let norm: f32 = centre.iter().map(|v| v * v).sum::<f32>().sqrt();
                        if norm > 0.0 {
                            centre.iter_mut().for_each(|slot| *slot /= norm);
                        }
                        (who.to_string(), centre)
                    })
                    .collect();
                ((*name).clone(), voices)
            })
            .collect();
        let oracle_candidates: Vec<score::Candidate> = oracle
            .iter()
            .flat_map(|(name, rows)| {
                rows.iter().map(|(speaker, vector)| score::Candidate {
                    speaker,
                    group: name,
                    vector,
                })
            })
            .collect();
        let oracle_pairs = cross_meeting_pairs(&oracle);
        println!(
            "\noracle voices  {} voices from perfect clustering — the ceiling any threshold aims at",
            oracle_candidates.len()
        );
        match score::equal_error_rate(&oracle_pairs) {
            Some((rate, at)) => println!("  cross-meeting EER {:.2}% at {at:.3}", rate * 100.0),
            None => println!("  cross-meeting EER not computable"),
        }
        match score::nearest_is_right(&oracle_candidates) {
            Some(rate) => println!("  nearest voice     {:.1}% right", rate * 100.0),
            None => println!("  nearest voice     not askable"),
        }
        match score::refusal_point(&oracle_pairs) {
            Some((at, refused)) => println!(
                "  floor that admits nobody: {at:.3}, refusing {:.2}% of genuine pairs",
                refused * 100.0
            ),
            None => println!("  floor that admits nobody: not askable"),
        }
        // The margin against the voices the product *means* to have. Read off
        // the pipeline's current voices it would be fitted to the
        // fragmentation instead of to the question it answers.
        let oracle_margins = score::margin_trials(&oracle_candidates);
        match score::equal_error_rate(&oracle_margins) {
            Some((rate, at)) => println!("  margin EER {:.2}% at {at:.3}", rate * 100.0),
            None => println!("  margin EER not computable"),
        }
        println!("  margin      wrong winner accepted   right winner refused");
        for point in score::curve(&oracle_margins, &margin_sweep) {
            println!(
                "    {:.2}        {:>6.2}%                {:>6.2}%{}",
                point.threshold,
                point.false_accept * 100.0,
                point.false_reject * 100.0,
                if (point.threshold - MATCH_MARGIN).abs() < 1e-6 {
                    "   <- MATCH_MARGIN"
                } else {
                    ""
                }
            );
        }

        // The same rebuild, but only the voices that would survive
        // `MIN_SPEAKER_MS` and become Speakers.
        //
        // **This is the control for the headline EER, and it has to be run
        // before that number means what it looks like it means.** The
        // measurement above scores every cluster the diarizer left standing,
        // including slivers holding a second or two; production mints a
        // Speaker only from a cluster with ten seconds of voice in it. If
        // the cross-meeting EER is being paid by fragments the product
        // already throws away, then the fragmentation is the harness's and
        // not the pipeline's, and the fix belongs in neither.
        //
        // Window count stands in for voiced milliseconds, which the cache
        // does not carry: windows advance a second at a time, so ten of them
        // span about ten seconds. A proxy, and named as one.
        const FLOOR_WINDOWS: usize = 10;
        let kept: Vec<(String, Labelled)> = windows
            .iter()
            .map(|(name, rows)| {
                let (voices, _) = voices_at_with_sizes(rows, MERGE_THRESHOLD, FLOOR_WINDOWS);
                ((*name).clone(), voices)
            })
            .collect();
        let kept_candidates: Vec<score::Candidate> = kept
            .iter()
            .flat_map(|(name, rows)| {
                rows.iter().map(|(speaker, vector)| score::Candidate {
                    speaker,
                    group: name,
                    vector,
                })
            })
            .collect();
        let kept_pairs = cross_meeting_pairs(&kept);
        println!(
            "\nvoices past the {FLOOR_WINDOWS}-window floor  {} of {} — what production would mint",
            kept_candidates.len(),
            windows
                .iter()
                .map(|(_, rows)| voices_at(rows, MERGE_THRESHOLD).0.len())
                .sum::<usize>()
        );
        match score::equal_error_rate(&kept_pairs) {
            Some((rate, at)) => println!("  cross-meeting EER {:.2}% at {at:.3}", rate * 100.0),
            None => println!("  cross-meeting EER not computable"),
        }
        match score::nearest_is_right(&kept_candidates) {
            Some(rate) => println!("  nearest voice     {:.1}% right", rate * 100.0),
            None => println!("  nearest voice     not askable"),
        }
        if let Some(rate) = score::false_accept_rate_at(&kept_pairs, MATCH_FLOOR) {
            println!(
                "  different colleagues above MATCH_FLOOR {MATCH_FLOOR}: {:.2}%",
                rate * 100.0
            );
        }

        // What each merge threshold costs where it is actually paid. The
        // curve above is about window pairs; this rebuilds the *voices* at
        // each candidate and asks the question the product asks — how many
        // voices came out, and does the right one win across meetings. A
        // threshold that splits one speaker into eleven fragments scores
        // well on neither, and the pairwise curve alone would not say so.
        // Over the same evenly spaced sample the pairwise curve used, not
        // the full windows: agglomeration is quadratic in them, and this
        // re-clusters the whole corpus once per candidate. The voice counts
        // below are therefore of the sample rather than of a real meeting —
        // it is the *trend* across thresholds that chooses one, and the
        // choice gets confirmed by a full re-run at the value it picks.
        println!(
            "\nmerge   voices   cross-meeting EER   nearest right   at floor   \
             purity   splinter   (over {WINDOW_SAMPLE}-window samples)"
        );
        for &threshold in &sweep {
            let scored: Vec<(String, Labelled, Quality)> = sampled
                .iter()
                .map(|(name, rows)| {
                    let (voices, quality) = voices_at(rows, threshold);
                    ((*name).clone(), voices, quality)
                })
                .collect();
            let rebuilt: Vec<(String, Labelled)> = scored
                .iter()
                .map(|(name, voices, _)| (name.clone(), voices.clone()))
                .collect();
            let candidates: Vec<score::Candidate> = rebuilt
                .iter()
                .flat_map(|(name, rows)| {
                    rows.iter().map(|(speaker, vector)| score::Candidate {
                        speaker,
                        group: name,
                        vector,
                    })
                })
                .collect();
            let pairs = cross_meeting_pairs(&rebuilt);
            let eer = score::equal_error_rate(&pairs);
            let nearest = score::nearest_is_right(&candidates);
            let at_floor = score::false_accept_rate_at(&pairs, MATCH_FLOOR);
            // Weighted by windows for purity, by meeting for splintering:
            // purity is a property of the windows, splintering of the room.
            let weight: usize = sampled.iter().map(|(_, rows)| rows.len()).sum();
            let purity = scored
                .iter()
                .zip(&sampled)
                .map(|((_, _, q), (_, rows))| q.purity * rows.len() as f64)
                .sum::<f64>()
                / weight.max(1) as f64;
            let splinter =
                scored.iter().map(|(_, _, q)| q.splinter).sum::<f64>() / scored.len().max(1) as f64;
            println!(
                "  {:.2}   {:>6}   {:>15}   {:>13}   {:>8}   {:>5.1}%   {:>6.1}x{}",
                threshold,
                candidates.len(),
                eer.map(|(rate, at)| format!("{:.2}% at {at:.2}", rate * 100.0))
                    .unwrap_or_else(|| "—".into()),
                nearest
                    .map(|rate| format!("{:.1}%", rate * 100.0))
                    .unwrap_or_else(|| "—".into()),
                at_floor
                    .map(|rate| format!("{:.2}%", rate * 100.0))
                    .unwrap_or_else(|| "—".into()),
                purity * 100.0,
                splinter,
                if (threshold - MERGE_THRESHOLD).abs() < 1e-6 {
                    "   <- MERGE_THRESHOLD"
                } else {
                    ""
                }
            );
        }
    }

    let total_seconds: f64 = measured.iter().map(|one| one.seconds).sum();
    let total_audio: f64 = measured.iter().map(|one| one.audio_seconds).sum();
    println!(
        "wall clock     {total_seconds:.1}s for {total_audio:.1}s of audio  \
         ({:.2}x real time)",
        total_seconds / total_audio.max(1.0)
    );

    // Bounds, not targets. The point of this binary is the numbers it
    // prints; asserting the recorded figure exactly would fail on the first
    // honest improvement, and asserting nothing would let a pipeline that
    // silently stopped producing turns pass. So: it ran, it produced
    // something, and the floor is below the real thing.
    if analyze_only {
        // These bounds are about a measurement this run deliberately did not
        // take. Asserting them anyway would turn the analysis path into a
        // failure every time it is used.
        return;
    }
    assert!(pooled.total_ms > 0, "no reference speech was scored at all");
    // **Confusion, not the rate.** The obvious assertion here was that the
    // oracle floor sits below the real rate, and the first real corpus run
    // failed it: on EN2002c the oracle scored 22.92% against a real 22.82%.
    //
    // That is not a bug in the pipeline, it is the oracle's definition
    // meeting overlapped speech. `oracle_relabel` gives each hypothesis span
    // the reference speaker who dominates it, independently — so two
    // hypothesis speakers talking at once can both be relabelled to the same
    // person, and the second one stops covering anybody. Real DER's optimal
    // mapping is one-to-one and cannot do that, so it keeps covering both,
    // even while naming one of them wrong. The oracle traded 159 s of extra
    // missed speech for 129 s less confusion and 27 s less false alarm.
    //
    // So "perfect clustering is at least as good" is false with overlap
    // scored and no collar, which is the protocol this file measures under.
    // What *is* true is the part the oracle exists to isolate: labelling
    // each span with its dominant speaker minimises that span's confusion,
    // and the real rate is constrained to one global mapping, so the
    // oracle's confusion can never be the larger of the two. That is also
    // the number ADR-0037 reads — the gap between the two confusions is what
    // clustering is costing.
    assert!(
        oracle.confusion_ms <= pooled.confusion_ms,
        "perfect clustering confused more speech than the real run, which \
         cannot happen: {oracle:?} vs {pooled:?}"
    );
    assert!(
        pooled.rate() < 1.0,
        "the pipeline attributed nothing usable: {pooled:?}"
    );
}

/// The two pure steps the threshold sweep leans on, checked without the
/// corpus so they run in the ordinary `cargo test` path.
///
/// Both are easy to get quietly wrong in ways no corpus run would report:
/// a labelling that picks the first speaker rather than the commonest, or a
/// rebuild that returns the windows it was given instead of the voices they
/// merge into, would still print a plausible table.
#[cfg(test)]
mod rebuilding_voices {
    use super::*;

    /// A unit vector at `angle` radians, so a test can place voices at a
    /// chosen cosine from one another.
    fn at(angle: f32) -> Vec<f32> {
        vec![angle.cos(), angle.sin()]
    }

    #[test]
    fn a_window_belongs_to_whoever_holds_most_of_it() {
        let reference = vec![
            Span::new("alice", 0, 1_000),
            Span::new("bob", 1_000, 10_000),
        ];
        assert_eq!(
            dominant_speaker(&[(0, 4_000)], &reference).as_deref(),
            Some("bob"),
            "three seconds of Bob outweigh one of Alice, even though Alice is first"
        );
        assert_eq!(
            dominant_speaker(&[(20_000, 24_000)], &reference),
            None,
            "a window over silence belongs to nobody rather than to the nearest guess"
        );
    }

    #[test]
    fn a_lower_threshold_merges_what_a_higher_one_keeps_apart() {
        // Two tight knots of windows, a wide angle apart. The merge
        // threshold is the only thing deciding whether they are one voice
        // or two, which is exactly the lever the sweep pulls.
        let mut windows: Labelled = Vec::new();
        for step in 0..4 {
            windows.push(("alice".into(), at(step as f32 * 0.01)));
            windows.push(("bob".into(), at(1.2 + step as f32 * 0.01)));
        }

        let apart = voices_at(&windows, 0.9).0;
        assert_eq!(
            apart.len(),
            2,
            "at a high bar they stay two voices: {apart:?}"
        );
        let mut named: Vec<&str> = apart.iter().map(|(who, _)| who.as_str()).collect();
        named.sort_unstable();
        assert_eq!(named, ["alice", "bob"], "and each is labelled by its own");

        let together = voices_at(&windows, 0.1).0;
        assert_eq!(
            together.len(),
            1,
            "at a low bar the whole room is one voice, which is the failure a \
             merge threshold that is too low produces: {together:?}"
        );
    }

    #[test]
    fn a_rebuilt_voice_is_a_unit_vector_so_cosines_stay_comparable() {
        let windows: Labelled = vec![
            ("alice".into(), vec![3.0, 0.0]),
            ("alice".into(), vec![0.0, 3.0]),
        ];
        let voices = voices_at(&windows, 0.0).0;
        assert_eq!(voices.len(), 1);
        let norm: f32 = voices[0].1.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-5,
            "an unnormalized centroid would make every cosine against it wrong: {norm}"
        );
    }
}
