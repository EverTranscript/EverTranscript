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

fn models() -> (PathBuf, PathBuf) {
    let dir = std::env::var_os("EVERTRANSCRIPT_MODELS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(evertranscript_core::paths::models_dir);
    let segmentation = dir.join("diarize-segmentation.onnx");
    let (name, file, _) = embedding_under_test();
    let embedding = dir.join(&file);
    println!("embedding under test: {name} ({})", file.display());
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
}

fn measure(meeting: &Meeting, segmentation: &Path, embedding: &Path) -> Measured {
    let samples = read_wav(&meeting.audio);
    let audio_seconds = samples.len() as f64 / diarize::fbank::SAMPLE_RATE as f64;
    let reference =
        score::parse_rttm(&std::fs::read_to_string(&meeting.reference).expect("read reference"));

    let mut diarizer =
        diarize::live::LiveDiarizer::load_with(segmentation, embedding, embedding_under_test().2)
            .expect("load models");
    let audio = diarize::MeetingAudio {
        mic: &samples,
        system: &[],
        sample_rate: diarize::fbank::SAMPLE_RATE,
    };

    // Wall clock per meeting, because a ceiling is one of the things being
    // fixed: clustering was cubic, and 70 s at 1,259 windows projected to a
    // quarter of an hour for a two-hour meeting.
    let started = Instant::now();
    let result =
        diarize::runner::run_guarded(&mut diarizer, audio, &mut |_| {}, &diarize::Cancel::new())
            .expect("diarize");
    let seconds = started.elapsed().as_secs_f64();

    let spans = hypothesis(&result.turns);
    Measured {
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

    let measured: Vec<Measured> = meetings
        .iter()
        .map(|meeting| {
            let one = measure(meeting, &segmentation, &embedding);
            // Per meeting, so one bad meeting is visible rather than
            // averaged into the corpus figure.
            println!(
                "{:<12}  DER {:>6.1}%  (missed {:>5.1}  false alarm {:>5.1}  confusion {:>5.1})  \
                 oracle {:>6.1}%  {:>6.1}s for {:>6.1}s of audio  ({:.2}x)",
                one.name,
                one.der.rate() * 100.0,
                one.der.missed_rate() * 100.0,
                one.der.false_alarm_rate() * 100.0,
                one.der.confusion_rate() * 100.0,
                one.oracle.rate() * 100.0,
                one.seconds,
                one.audio_seconds,
                one.seconds / one.audio_seconds.max(1.0),
            );
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

    let trials = cross_meeting_trials(&measured);
    let eer = score::equal_error_rate(&trials);
    let false_accepts = score::false_accept_rate_at(&trials, MATCH_FLOOR);

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
    assert!(pooled.total_ms > 0, "no reference speech was scored at all");
    assert!(
        oracle.rate() <= pooled.rate() + 1e-9,
        "the oracle floor is above the real rate, which cannot happen: \
         {oracle:?} vs {pooled:?}"
    );
    assert!(
        pooled.rate() < 1.0,
        "the pipeline attributed nothing usable: {pooled:?}"
    );
}
