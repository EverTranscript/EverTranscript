//! The Summary sidecar loading a real model and generating real tokens.
//!
//! M4 verified this by hand, on macOS, once. Two criteria have carried the
//! same sentence ever since — that the sidecar *cross-compiles* for Windows,
//! which M3 established is worth nothing about runtime. This is the test that
//! makes the claim checkable on whatever platform it runs on, so Windows can
//! stop being a place the binary has only ever been linked.
//!
//! Needs the registered model. Set `EVERTRANSCRIPT_SUMMARY_MODEL` to a
//! `.gguf`; without it these skip rather than fail, so a machine that has not
//! downloaded half a gigabyte still runs a green suite — the same bargain
//! `transcription_quality.rs` makes.
//!
//! **What this does not assert is quality.** The registered 0.5B is the model
//! that was verified, not the model that should ship, and M4 measured it
//! getting two plain action items wrong. Asserting correctness here would
//! either fail on a known-weak model or quietly lock that weakness in as the
//! expectation. What is asserted is that inference *happened*: a model
//! loaded, tokens came back, and they are not the prompt handed back.

use std::path::Path;
use std::path::PathBuf;

use evertranscript_core::summary::Backend;
use evertranscript_core::summary::BackendIdentity;
use evertranscript_core::summary::Cancel;
use evertranscript_core::summary::Request;
use evertranscript_core::summary::prompt::DEFAULT_SYSTEM_PROMPT;
use evertranscript_core::summary::sidecar::SidecarBackend;

/// A transcript with one unambiguous commitment in it.
///
/// Deliberately short: this test pays for a model load on every run and the
/// thing being proven is that generation happens at all, not what it costs.
const TRANSCRIPT: &str = "\
Frank: The migration is blocked on the staging database being out of date.
Priya: I'll refresh staging from last night's snapshot before Thursday.
Frank: Thanks. Then we can cut over on Friday morning.";

/// The model, or `None` only when nobody asked for one.
///
/// **A set variable pointing at nothing is a failure, not a skip.** The
/// obvious spelling — `var(..).ok()?` then `path.exists().then_some(path)` —
/// turns a typo'd path, a failed download or a renamed artifact into a silent
/// green tick, and CI sets this variable on every run, so the whole test
/// would report success for work it never did. That is exactly the vacuous
/// pass DECISIONS Q43 had to go back and correct in two guarantee tests, and
/// the correction is worth applying before it is earned rather than after.
fn model() -> Option<PathBuf> {
    let configured = std::env::var_os("EVERTRANSCRIPT_SUMMARY_MODEL")?;
    let path = PathBuf::from(configured);
    assert!(
        path.exists(),
        "EVERTRANSCRIPT_SUMMARY_MODEL points at {}, which does not exist — \
         a set-but-missing model must fail rather than skip, or this test \
         reports green without loading anything",
        path.display()
    );
    Some(path)
}

/// The sidecar binary, beside this test's own executable.
///
/// `EVERTRANSCRIPT_SUMMARIZER_BIN` first, matching what the Core itself reads
/// (`server.rs`), so one override works for both. Otherwise `target/<profile>/`
/// — an integration test runs from `target/<profile>/deps/`, and `--workspace`
/// has built the sidecar into its parent.
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

/// Both, or a reason. **A present model and an absent binary is a failure**,
/// not a skip: someone asked for this to run, and skipping would report green
/// for work that did not happen — which is the exact shape of the vacuous
/// guarantee tests DECISIONS Q43 had to correct.
fn spawn() -> Option<SidecarBackend> {
    let model = model()?;
    let binary = sidecar().expect(
        "EVERTRANSCRIPT_SUMMARY_MODEL is set, so the sidecar was meant to run, \
         but the binary is not beside this test — build it with \
         `cargo build -p evertranscript-summarizer` or set \
         EVERTRANSCRIPT_SUMMARIZER_BIN",
    );
    let model = model.to_str().expect("the model path should be UTF-8");
    Some(SidecarBackend::spawn(Path::new(&binary), model).expect("the model should load"))
}

#[test]
fn the_sidecar_loads_a_model_and_generates() {
    let Some(mut backend) = spawn() else {
        eprintln!("skipping: EVERTRANSCRIPT_SUMMARY_MODEL is not set");
        return;
    };

    // The model that answered, named. On the platform this test exists for,
    // the interesting line in the log is this one.
    let BackendIdentity::LocalSidecar { model } = backend.identity() else {
        panic!("a sidecar Backend must identify as local");
    };
    assert!(
        !model.is_empty(),
        "the sidecar should report which model it loaded"
    );
    eprintln!("loaded: {model}");

    let request = Request {
        system: DEFAULT_SYSTEM_PROMPT.to_string(),
        user: evertranscript_core::summary::prompt::build_user_message(None, TRANSCRIPT),
    };
    let summary = backend
        .generate(&request, &Cancel::new())
        .expect("generation should succeed");

    eprintln!("generated {} chars:\n{summary}", summary.len());

    let trimmed = summary.trim();
    assert!(!trimmed.is_empty(), "the model generated nothing");
    // Long enough to be a decode rather than a single stop token. Not a
    // quality bar — a liveness one.
    assert!(
        trimmed.chars().count() > 20,
        "the model produced {} characters, which is not a generation: {trimmed:?}",
        trimmed.chars().count()
    );

    // **Prompt scaffolding must not survive.** The stop sequences are the only
    // thing standing between a model that replays its prompt and a Meeting's
    // permanent record, and this is the assertion that keeps them honest —
    // `</transcript>` was missing from that list until this test was first run
    // against a real model (DECISIONS Q45). Nothing legitimate can produce
    // these: `escape_control_markers` breaks both tags in every untrusted
    // string, so a literal one here was written by the model.
    for scaffolding in ["</transcript>", "<transcript>", "The operator's own notes"] {
        assert!(
            !summary.contains(scaffolding),
            "prompt scaffolding {scaffolding:?} reached the output:\n{summary}"
        );
    }
    assert!(
        !summary.contains(DEFAULT_SYSTEM_PROMPT),
        "the model echoed its own system prompt:\n{summary}"
    );

    // **Reported, not asserted.** How much of the transcript came back
    // verbatim is a quality measurement, and on the registered 0.5B it is
    // high — the model reproduces the input and invents timestamps for it
    // (Q45). Failing here would make a platform test fail for a model's
    // sake on every platform at once, and would quietly turn M4's open
    // "choose a real default model" criterion into this test's problem. So
    // the number is printed on every run and the criterion stays where it
    // belongs.
    let echoed = TRANSCRIPT
        .lines()
        .filter(|line| !line.trim().is_empty() && summary.contains(line.trim()))
        .count();
    eprintln!(
        "transcript lines reproduced verbatim: {echoed}/{}",
        TRANSCRIPT.lines().count()
    );

    // Still answering afterwards. A sidecar that generates once and then
    // wedges is a sidecar that works exactly one time per Meeting.
    assert!(
        backend.ping(),
        "the sidecar stopped answering after generating"
    );
    backend.shutdown();
}

#[test]
fn a_loaded_sidecar_shuts_down_rather_than_hanging() {
    let Some(mut backend) = spawn() else {
        eprintln!("skipping: EVERTRANSCRIPT_SUMMARY_MODEL is not set");
        return;
    };

    // With a model resident, not just after a bare spawn. The hang M4 found
    // was a `shutdown` waiting for a reply from a child that was busy, and a
    // child holding half a gigabyte is the one that matters — an orphan here
    // costs the Operator their memory.
    assert!(backend.ping());
    backend.shutdown();
}
