//! Whether the registered Summary model is good enough to be the default.
//!
//! M4's close-out owes a criterion: *choose the real default by measurement
//! rather than reputation*. DECISIONS Q45 measured the incumbent and found it
//! wanting, and the evidence lived in prose — which is how a criterion stays
//! open for a milestone. This is that measurement as a standing test, so the
//! next swap inherits a bar instead of an anecdote.
//!
//! **Separate from `summary_inference` on purpose.** That test's subject is
//! the *platform* — it proves a model loads and generates on whatever is
//! running it, and it reports quality without asserting, because a platform
//! test must not go red for a model's sake. This test's subject is the
//! *model*, and it is allowed to fail when the model is bad.
//!
//! ## The bar (DECISIONS Q31)
//!
//! Three axes, and they are not equal:
//!
//! 1. **Fabricated timestamps: zero. A gate, not a score.** Missing an action
//!    item leaves the record incomplete, which the product already admits to.
//!    Inventing a `Said at` puts a false claim *into* a record ADR-0009 makes
//!    permanent, in the column whose stated purpose is letting an item be
//!    checked against what was actually said.
//! 2. Action items found must improve on the incumbent.
//! 3. Verbatim echo must improve on the incumbent.
//!
//! ## The input is production-shaped, and that matters
//!
//! `render_transcript` always emits `[HH:MM:SS] Speaker: text`, so a real
//! prompt always carries the timestamps the `Said at` column asks for. Q45's
//! measurement — and the first version of this one — used a fixture with no
//! timestamps at all, then recorded the model inventing them. Given nothing
//! to cite, both models invent; given the real thing, Qwen3 cites correctly.
//! **The fixture was measuring the harness.** A transcript here therefore
//! looks like what the Core actually builds.
//!
//! Needs the registered model in `EVERTRANSCRIPT_SUMMARY_MODEL`. Unset skips;
//! set-but-missing fails, because CI sets it and a silent skip would report
//! success for work that never happened (DECISIONS Q43).

#![cfg(unix)]

use std::path::Path;
use std::path::PathBuf;

use evertranscript_core::summary::Backend;
use evertranscript_core::summary::Cancel;
use evertranscript_core::summary::Request;
use evertranscript_core::summary::prompt::DEFAULT_SYSTEM_PROMPT;
use evertranscript_core::summary::sidecar::Driving;
use evertranscript_core::summary::sidecar::SidecarBackend;

/// As the Core renders it: timestamps, speaker labels, one line per segment.
const TRANSCRIPT: &str = "\
[00:00:04] Frank: The migration is blocked on the staging database being out of date.
[00:00:11] Priya: I'll refresh staging from last night's snapshot before Thursday.
[00:00:19] Frank: Thanks. Then we can cut over on Friday morning.
";

/// What the incumbent scored on exactly this input, driven as it shipped —
/// plain framing, greedy decoding. Measured, not remembered.
const INCUMBENT_ACTION_ITEMS: usize = 0;
const INCUMBENT_VERBATIM_LINES: usize = 3;

/// Separates a rendered line's timestamp from its speech, so echo is measured
/// on what was *said* rather than on the bracket the Core printed.
const TIMESTAMP_END: &str = "] ";

/// Where the quality of a model is measured.
///
/// **Separate from having a model, because they are different questions.**
/// `EVERTRANSCRIPT_SUMMARY_MODEL` says a model is available, and every
/// platform that ships one should prove it loads and generates there — that
/// is `summary_inference`, and it is per-platform on purpose (DECISIONS Q45).
/// How *good* the model is does not vary by platform, so measuring it twice
/// buys nothing and costs a second multi-gigabyte load, which timed a CI job
/// out at forty-five minutes.
const MEASURE_ENV: &str = "EVERTRANSCRIPT_MEASURE_SUMMARY_QUALITY";

fn model() -> Option<PathBuf> {
    std::env::var_os(MEASURE_ENV).filter(|value| !value.is_empty())?;
    // **Empty is unset.** A workflow that supplies this conditionally sets it
    // to the empty string on the platforms that skip, and `var_os` reports
    // that as present — so the set-but-missing assertion below fired on the
    // very platform meant to opt out. An empty path is not a path, which is
    // the same thing ticket 01 settled for empty Meeting names.
    let configured = std::env::var_os("EVERTRANSCRIPT_SUMMARY_MODEL")
        .filter(|value| !value.is_empty())
        .expect("set EVERTRANSCRIPT_SUMMARY_MODEL to measure a model's quality");
    let path = PathBuf::from(configured);
    assert!(
        path.exists(),
        "EVERTRANSCRIPT_SUMMARY_MODEL points at {}, which does not exist — \
         a set-but-missing model must fail rather than skip",
        path.display()
    );
    Some(path)
}

fn sidecar() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("EVERTRANSCRIPT_SUMMARIZER_BIN") {
        let path = PathBuf::from(path);
        return path.exists().then_some(path);
    }
    let name = if cfg!(windows) {
        "evertranscript-summarizer.exe"
    } else {
        "evertranscript-summarizer"
    };
    let beside = std::env::current_exe().ok()?.parent()?.parent()?.join(name);
    beside.exists().then_some(beside)
}

/// One Summary of [`TRANSCRIPT`], generated once for the whole binary.
///
/// **Loaded once on purpose.** Each of the assertions below is about the same
/// output, and spawning a sidecar per test meant loading 2.5 GB three times
/// over — which timed the CI job out at forty-five minutes. Three questions
/// about one Summary is also the more honest shape: they are not independent
/// experiments, they are three properties of a single answer.
fn summary() -> Option<&'static str> {
    static SUMMARY: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    SUMMARY.get_or_init(summarize).as_deref()
}

fn summarize() -> Option<String> {
    let model = model()?;
    let binary = sidecar().expect("the sidecar must be built to measure the model it loads");
    let driving = evertranscript_core::models::registry::SUMMARY_DEFAULT
        .driving
        .as_ref()
        .map(Driving::from_entry);
    let mut backend = SidecarBackend::spawn_driven(
        Path::new(&binary),
        model.to_str().expect("the model path should be UTF-8"),
        driving,
    )
    .expect("the model should load");

    let request = Request {
        system: DEFAULT_SYSTEM_PROMPT.to_string(),
        user: evertranscript_core::summary::prompt::build_user_message(None, TRANSCRIPT),
    };
    let text = backend
        .generate(&request, &Cancel::new())
        .expect("generation should succeed");
    backend.shutdown();
    // Through the same scrub the record gets, so this measures what an
    // Operator would read rather than the raw decode.
    Some(evertranscript_core::summary::prompt::scrub(&text))
}

/// Every `H:MM`-shaped run in the text.
///
/// Deliberately loose: `09:30`, `00:00:11` and `9:30` all match, because the
/// question is whether the model *stated a time*, not whether it stated one
/// in a particular format.
fn times_in(text: &str) -> Vec<String> {
    let bytes: Vec<char> = text.chars().collect();
    let mut found = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_digit() {
            let start = index;
            let mut has_colon = false;
            while index < bytes.len() && (bytes[index].is_ascii_digit() || bytes[index] == ':') {
                has_colon |= bytes[index] == ':';
                index += 1;
            }
            if has_colon {
                found.push(bytes[start..index].iter().collect());
            }
        } else {
            index += 1;
        }
    }
    found
}

#[test]
fn the_summary_model_never_invents_a_timestamp() {
    let Some(summary) = summary() else {
        eprintln!("skipping: set {MEASURE_ENV} to measure the registered model");
        return;
    };
    eprintln!("summary:\n{summary}");

    // **The gate.** Every time the Summary states must be one the transcript
    // states. A `Said at` the meeting never contained is a false claim in a
    // record that cannot be edited — worse than an incomplete one, because it
    // is the column an Operator would use to check the others.
    let invented: Vec<String> = times_in(summary)
        .into_iter()
        .filter(|time| !TRANSCRIPT.contains(time.as_str()))
        .collect();
    assert!(
        invented.is_empty(),
        "the Summary states times the transcript does not contain: {invented:?}\n\
         The transcript's only times are 00:00:04, 00:00:11 and 00:00:19.\n\n{summary}"
    );
}

#[test]
fn the_summary_model_finds_the_commitments_that_were_made() {
    let Some(summary) = summary() else {
        eprintln!("skipping: set {MEASURE_ENV} to measure the registered model");
        return;
    };

    // Two commitments in three lines: Priya will refresh staging, and the
    // cut-over happens Friday. Counted by whether the Summary names them at
    // all, which is the loosest honest reading — this is a floor, not a score.
    let found = ["refresh", "cut over"]
        .iter()
        .filter(|needle| summary.to_lowercase().contains(*needle))
        .count();
    eprintln!("action items named: {found}/2 (incumbent scored {INCUMBENT_ACTION_ITEMS})");
    assert!(
        found > INCUMBENT_ACTION_ITEMS,
        "the registered model must find more of the commitments than the model it \
         replaced, which found {INCUMBENT_ACTION_ITEMS}:\n\n{summary}"
    );
}

#[test]
fn the_summary_model_summarizes_rather_than_reproducing() {
    let Some(summary) = summary() else {
        eprintln!("skipping: set {MEASURE_ENV} to measure the registered model");
        return;
    };

    // The incumbent reproduced every line of the transcript verbatim, which
    // is not a summary — it is a copy with a heading.
    let echoed = TRANSCRIPT
        .lines()
        .filter(|line| {
            let spoken = line
                .split_once(TIMESTAMP_END)
                .map_or(*line, |(_, rest)| rest);
            !spoken.trim().is_empty() && summary.contains(spoken.trim())
        })
        .count();
    eprintln!(
        "lines reproduced verbatim: {echoed}/3 (incumbent reproduced {INCUMBENT_VERBATIM_LINES})"
    );
    assert!(
        echoed < INCUMBENT_VERBATIM_LINES,
        "the registered model reproduces as much of the transcript as the one it \
         replaced ({INCUMBENT_VERBATIM_LINES} of 3):\n\n{summary}"
    );
}
