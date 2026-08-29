//! The product's promises, as tests against the shipped binary.
//!
//! EverTranscript's pitch is two by-construction guarantees and an
//! open-source posture that invites you to check them. A promise nothing
//! verifies is marketing, so each one here runs against the real artifact
//! rather than against intentions:
//!
//! - **No telemetry** (ADR-0034): not disabled, *absent*. The binary is
//!   scanned for analytics and crash-reporting SDKs.
//! - **Sanctioned Traffic** (ADR-0034): with models present, a full record →
//!   stop → Mirror cycle opens no network connections at all.
//! - **The permission set** (ADR-0027/0030/0036): microphone and
//!   system-audio only. Screen Recording and Calendars appear solely behind
//!   opt-in features that do not exist yet in M1.
//! - **No secrets in the record** (story 41): nothing key-shaped in the
//!   database, the Mirrors, or the logs.

use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

/// The binary under test, built by cargo for this integration test.
fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_evertranscript"))
}

/// Runs a CLI command against a scratch installation.
fn run(history: &Path, runtime: &Path, args: &[&str]) -> std::process::Output {
    Command::new(binary())
        .args(args)
        .env("EVERTRANSCRIPT_HISTORY_DIR", history)
        .env("EVERTRANSCRIPT_RUNTIME_DIR", runtime)
        .output()
        .expect("running the CLI")
}

/// Waits until the Core is answering, rather than guessing how long it takes.
///
/// These tests used a fixed 1500 ms sleep, which is a claim about how fast
/// the machine is: true on a warm laptop and false on a cold CI runner,
/// where the Core had not bound its socket yet, every following command
/// failed quietly, and the assertion blamed the History folder for being
/// empty. Polling asks the question the sleep was standing in for.
fn wait_for_core(history: &Path, runtime: &Path) {
    for _ in 0..120 {
        if run(history, runtime, &["status"]).status.success() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    panic!("the Core never started answering");
}

/// Which network connections a process holds, as this platform reports them.
///
/// `lsof` does not exist on Windows, and the guarantee it serves — that a
/// full recording cycle opens nothing — matters equally on both platforms,
/// so it is asked for in each platform's own dialect rather than skipped on
/// one of them.
fn open_sockets(pid: u32) -> String {
    #[cfg(windows)]
    let output = Command::new("netstat").args(["-ano"]).output();
    #[cfg(not(windows))]
    let output = Command::new("lsof")
        .args(["-p", &pid.to_string(), "-i", "-a", "-n", "-P"])
        .output();

    let Ok(output) = output else {
        // A missing tool must not read as "no connections": that would turn
        // a guarantee into a tautology on any machine without it.
        panic!("could not ask this platform what sockets are open");
    };
    let text = String::from_utf8_lossy(&output.stdout).to_string();

    #[cfg(windows)]
    {
        // netstat reports the whole machine; keep only our own rows.
        let needle = format!(" {pid}");
        text.lines()
            .filter(|line| line.ends_with(&needle))
            .collect::<Vec<_>>()
            .join("\n")
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        text
    }
}

/// Every string that would betray an analytics or crash-reporting SDK.
///
/// All three competitors ship at least one of these. "No analytics SDK
/// exists in the binary" is a sentence they structurally cannot say, and
/// this test is what keeps it true for us.
const FORBIDDEN_SDKS: &[&str] = &[
    "sentry.io",
    "ingest.sentry",
    "posthog.com",
    "amplitude.com",
    "api.segment.io",
    "mixpanel.com",
    "bugsnag.com",
    "datadoghq.com",
    "google-analytics.com",
    "firebaseio.com",
    "statsig.com",
    "crashlytics",
];

/// Hosts the product may never contact. Cloud ASR and cloud calendars are
/// foreclosed by ADR-0002 and ADR-0036 respectively; finding one of these in
/// the binary would mean a path exists.
const FORBIDDEN_HOSTS: &[&str] = &[
    "api.deepgram.com",
    "streaming.assemblyai.com",
    "api.assemblyai.com",
    "speech.googleapis.com",
    "api.pyannote.ai",
    "www.googleapis.com/calendar",
    "graph.microsoft.com",
];

fn binary_strings() -> String {
    let output = Command::new("strings")
        .arg(binary())
        .output()
        .expect("running strings");
    String::from_utf8_lossy(&output.stdout).to_lowercase()
}

#[test]
fn the_binary_contains_no_analytics_or_crash_reporting_sdk() {
    let contents = binary_strings();
    let found: Vec<&str> = FORBIDDEN_SDKS
        .iter()
        .copied()
        .filter(|needle| contents.contains(&needle.to_lowercase()))
        .collect();
    assert!(
        found.is_empty(),
        "ADR-0034 says telemetry is absent, not disabled — but the binary mentions {found:?}"
    );
}

#[test]
fn the_binary_knows_no_cloud_transcription_or_calendar_endpoint() {
    // The Closed Boundary is the absence of a path, not a guarded one. A
    // hostname in the binary is a path.
    let contents = binary_strings();
    let found: Vec<&str> = FORBIDDEN_HOSTS
        .iter()
        .copied()
        .filter(|needle| contents.contains(&needle.to_lowercase()))
        .collect();
    assert!(
        found.is_empty(),
        "these endpoints must not exist in the binary at all: {found:?}"
    );
}

#[test]
#[cfg(target_os = "macos")]
fn the_binary_links_no_screen_capture_or_location_framework() {
    // ADR-0027 removed Screen Recording from the sanctioned permission set.
    // EventKit joined in M2 under ADR-0036 and is now expected; everything
    // it dragged in with it is not.
    //
    // This test earned its place the moment the calendar landed. Adding
    // `objc2-event-kit` with its default features linked **MapKit and
    // CoreLocation** — a location framework, in a product whose entire
    // claim is that it processes no input it was not handed. Nothing in the
    // code referenced either one; they arrived through a default feature
    // set, which is exactly the kind of thing no one reads a diff for.
    // `default-features = false` removed them, and this assertion is what
    // keeps them gone.
    let output = Command::new("otool")
        .args(["-L"])
        .arg(binary())
        .output()
        .expect("running otool");
    let linked = String::from_utf8_lossy(&output.stdout);

    for framework in ["ScreenCaptureKit", "Contacts", "MapKit", "CoreLocation"] {
        assert!(
            !linked.contains(framework),
            "nothing sanctions linking {framework}:\n{linked}"
        );
    }
    // And it does link what it legitimately needs for the microphone.
    assert!(
        linked.contains("CoreAudio") || linked.contains("AudioToolbox"),
        "microphone capture should link CoreAudio:\n{linked}"
    );

    // AppKit, for the menu bar item (ADR-0023), brings CloudKit and CoreData
    // with it. Anyone auditing this binary's frameworks will see them and
    // should know why: they arrive as AppKit's own dependencies and nothing
    // here calls into them. Linking is not using, and the claim that matters
    // is proved by `a_full_recording_cycle_opens_no_network_connections`
    // below, which watches actual sockets rather than the linker.
    if linked.contains("CloudKit") {
        assert!(
            linked.contains("AppKit"),
            "CloudKit is tolerated only as AppKit's dependency, and AppKit is absent:\n{linked}"
        );
    }
}

#[test]
fn a_full_recording_cycle_opens_no_network_connections() {
    // The Sanctioned Traffic claim, checked rather than asserted: with
    // models already present (or absent — either way nothing is fetched),
    // recording and stopping must touch the network zero times.
    let dir = tempfile::tempdir().expect("tempdir");
    let history = dir.path().join("History");
    let runtime = dir.path().join("run");

    let mut daemon = Command::new(binary())
        .arg("daemon")
        .env("EVERTRANSCRIPT_HISTORY_DIR", &history)
        .env("EVERTRANSCRIPT_RUNTIME_DIR", &runtime)
        // No menu bar item: these tests assert on a binary, and they also
        // stand in as the regression test for the headless daemon path.
        .env(evertranscript_core::tray::DISABLE_ENV, "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("starting the Core");
    wait_for_core(&history, &runtime);

    run(&history, &runtime, &["acknowledge"]);
    run(&history, &runtime, &["record", "start", "--app", "Zoom"]);
    std::thread::sleep(std::time::Duration::from_millis(500));
    run(&history, &runtime, &["record", "stop"]);

    // lsof is the ground truth for what this process actually has open —
    // read while the Core is still alive, before it is killed below.
    let outcome = open_sockets(daemon.id());

    let _ = daemon.kill();
    let _ = daemon.wait();

    let connections: Vec<&str> = outcome
        .lines()
        .skip(1) // header
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert!(
        connections.is_empty(),
        "recording must produce no network traffic, but the Core had these sockets open:\n{}",
        connections.join("\n")
    );
}

#[test]
fn nothing_key_shaped_reaches_the_record_or_the_logs() {
    // Story 41: secrets live only in the OS credential store, never in the
    // database, the Mirrors, or the logs. M1 holds no keys at all, and this
    // is what keeps that true as cloud Summary arrives in M4.
    let dir = tempfile::tempdir().expect("tempdir");
    let history = dir.path().join("History");
    let runtime = dir.path().join("run");

    let mut daemon = Command::new(binary())
        .arg("daemon")
        .env("EVERTRANSCRIPT_HISTORY_DIR", &history)
        .env("EVERTRANSCRIPT_RUNTIME_DIR", &runtime)
        // No menu bar item: these tests assert on a binary, and they also
        // stand in as the regression test for the headless daemon path.
        .env(evertranscript_core::tray::DISABLE_ENV, "1")
        .env("EVERTRANSCRIPT_LOG", "evertranscript_core=debug")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("starting the Core");
    wait_for_core(&history, &runtime);
    run(&history, &runtime, &["acknowledge"]);
    run(&history, &runtime, &["record", "start", "--app", "Zoom"]);
    std::thread::sleep(std::time::Duration::from_millis(400));
    run(&history, &runtime, &["record", "stop"]);
    let _ = daemon.kill();
    let output = daemon.wait_with_output().expect("collecting output");

    // Prefixes of the credential formats the M4 providers actually use.
    let key_shapes = ["sk-", "sk-ant-", "xoxb-", "AKIA", "ghp_", "Bearer "];
    let mut scanned = Vec::new();

    for entry in walk(&history) {
        if let Ok(contents) = std::fs::read_to_string(&entry) {
            scanned.push((entry.display().to_string(), contents));
        }
    }
    scanned.push((
        "stdout".into(),
        String::from_utf8_lossy(&output.stdout).to_string(),
    ));
    scanned.push((
        "stderr".into(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    ));

    for (where_, contents) in scanned {
        for shape in key_shapes {
            assert!(
                !contents.contains(shape),
                "{where_} contains something key-shaped ({shape})"
            );
        }
    }
}

#[test]
fn a_recording_survives_the_core_being_killed() {
    // The consolidated crash property: a Core killed mid-meeting leaves a
    // Meeting that is recoverable, not a corrupt store.
    let dir = tempfile::tempdir().expect("tempdir");
    let history = dir.path().join("History");
    let runtime = dir.path().join("run");

    let mut daemon = Command::new(binary())
        .arg("daemon")
        .env("EVERTRANSCRIPT_HISTORY_DIR", &history)
        .env("EVERTRANSCRIPT_RUNTIME_DIR", &runtime)
        // No menu bar item: these tests assert on a binary, and they also
        // stand in as the regression test for the headless daemon path.
        .env(evertranscript_core::tray::DISABLE_ENV, "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("starting the Core");
    wait_for_core(&history, &runtime);
    run(&history, &runtime, &["acknowledge"]);
    run(&history, &runtime, &["record", "start", "--app", "Zoom"]);
    std::thread::sleep(std::time::Duration::from_millis(500));

    // No graceful stop: kill it exactly as a crash would.
    let _ = daemon.kill();
    let _ = daemon.wait();

    // A fresh Core must come up cleanly on the same store and see the
    // Meeting, rather than refusing to start or losing it.
    let mut restarted = Command::new(binary())
        .arg("daemon")
        .env("EVERTRANSCRIPT_HISTORY_DIR", &history)
        .env("EVERTRANSCRIPT_RUNTIME_DIR", &runtime)
        // No menu bar item: these tests assert on a binary, and they also
        // stand in as the regression test for the headless daemon path.
        .env(evertranscript_core::tray::DISABLE_ENV, "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("restarting the Core");
    wait_for_core(&history, &runtime);

    let listed = run(&history, &runtime, &["list", "--json"]);
    let _ = restarted.kill();
    let _ = restarted.wait();

    let text = String::from_utf8_lossy(&listed.stdout);
    assert!(
        text.contains("\"id\""),
        "the Meeting must survive a crash and be listed after restart:\n{text}"
    );
}

#[test]
fn the_history_folder_holds_only_notes_and_a_hidden_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let history = dir.path().join("History");
    let runtime = dir.path().join("run");

    let mut daemon = Command::new(binary())
        .arg("daemon")
        .env("EVERTRANSCRIPT_HISTORY_DIR", &history)
        .env("EVERTRANSCRIPT_RUNTIME_DIR", &runtime)
        // No menu bar item: these tests assert on a binary, and they also
        // stand in as the regression test for the headless daemon path.
        .env(evertranscript_core::tray::DISABLE_ENV, "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("starting the Core");
    wait_for_core(&history, &runtime);
    run(&history, &runtime, &["acknowledge"]);
    run(&history, &runtime, &["record", "start", "--app", "Zoom"]);
    std::thread::sleep(std::time::Duration::from_millis(300));
    run(&history, &runtime, &["record", "stop"]);
    let _ = daemon.kill();
    let _ = daemon.wait();

    let visible: Vec<String> = std::fs::read_dir(&history)
        .expect("read history")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| !name.starts_with('.'))
        .collect();

    assert!(
        visible.iter().all(|name| name.ends_with(".md")),
        "the folder must read as meeting notes (ADR-0035): {visible:?}"
    );
    assert!(
        history.join(".data").is_dir(),
        "the machine store belongs in the hidden .data folder"
    );
}

/// Every file under a directory, recursively.
fn walk(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(walk(&path));
        } else {
            found.push(path);
        }
    }
    found
}
