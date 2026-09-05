//! EverTranscript: one binary that is both the Core daemon and the CLI.
//!
//! `evertranscript daemon` runs the Core — the login item that detects,
//! captures, transcribes, and stores. Every other subcommand is a Client of
//! a running Core over the local protocol (ADR-0026): the CLI never touches
//! the record directly.

use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use clap::Subcommand;
use evertranscript_core::client::CoreClient;
use evertranscript_core::paths;
use evertranscript_protocol::BriefingResponse;
use evertranscript_protocol::ChineseScript;
use evertranscript_protocol::DiarizeState;
use evertranscript_protocol::DiarizeStatusResponse;
use evertranscript_protocol::HistorySearchResponse;
use evertranscript_protocol::Meeting;
use evertranscript_protocol::MeetingDeleteResponse;
use evertranscript_protocol::MeetingDetailResponse;
use evertranscript_protocol::MeetingExportResponse;
use evertranscript_protocol::MeetingListResponse;
use evertranscript_protocol::MeetingResponse;
use evertranscript_protocol::ModelAvailability;
use evertranscript_protocol::ModelsStatusResponse;
use evertranscript_protocol::SettingsResponse;
use evertranscript_protocol::SpeakerDetailResponse;
use evertranscript_protocol::SpeakerListResponse;
use evertranscript_protocol::SpeakerResponse;
use evertranscript_protocol::SummaryBackendsResponse;
use evertranscript_protocol::TranscriptReassignResponse;
use evertranscript_protocol::TranscriptSnapshotResponse;
use evertranscript_protocol::WatchlistKind;
use evertranscript_protocol::WatchlistResponse;
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(
    name = "evertranscript",
    version,
    about = "A local-first meeting notetaker that never misses a meeting.",
    long_about = None,
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the Core in the foreground (the login item runs this).
    Daemon,
    /// Report the running Core's version, uptime, and state.
    Status {
        /// Print raw JSON instead of a human summary.
        #[arg(long)]
        json: bool,
    },
    /// Print the paths this build uses.
    Paths {
        #[arg(long)]
        json: bool,
    },
    /// Start and stop recording.
    #[command(subcommand)]
    Record(RecordCommand),
    /// List Meetings, most recent first.
    List {
        #[arg(long, default_value_t = 20)]
        limit: u32,
        #[arg(long)]
        json: bool,
    },
    /// Show one Meeting and its Transcript.
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Rename a Meeting. Its Mirror is renamed to match.
    Retitle { id: String, title: String },
    /// Delete a Meeting entirely — rows, Mirror, and audio.
    Delete {
        id: String,
        /// Skip the confirmation prompt.
        #[arg(long)]
        force: bool,
    },
    /// Search History.
    Search {
        query: Vec<String>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
        #[arg(long)]
        json: bool,
    },
    /// Print a Meeting's Mirror markdown to stdout.
    Export { id: String },
    /// Inspect and fetch the models the Core needs.
    #[command(subcommand)]
    Models(ModelsCommand),
    /// See and edit what Meeting Detection watches.
    #[command(subcommand)]
    Watchlist(WatchlistCommand),
    /// Follow live captions from the Meeting in progress.
    Captions,
    /// Show this installation's settings, and change the ones that are
    /// preferences rather than consent.
    Settings {
        #[arg(long)]
        json: bool,
        /// Which Han script Mandarin is recorded in. Takes effect for the
        /// next Meeting: the running one chose when it started, and a
        /// transcript written two ways would be worse.
        #[arg(long, value_parser = ["simplified", "traditional"])]
        chinese_script: Option<String>,
    },
    /// Acknowledge the first-run briefing. Nothing is captured before this.
    Acknowledge,
    /// Check that this machine can actually record, by recording.
    ///
    /// Runs without the Core, so it works before anything is installed.
    AudioCheck {
        /// Seconds to listen for. Longer is more conclusive: a refused
        /// system-audio permission looks exactly like nobody talking until
        /// enough audio has been played to prove otherwise.
        #[arg(long, default_value_t = evertranscript_core::audio::check::DEFAULT_SECONDS)]
        seconds: u64,
    },
    /// Read and write a Meeting's Notes — your own writing, editable
    /// forever, and what steers the Summary.
    #[command(subcommand)]
    Notes(NotesCommand),
    /// Generate and read a Meeting's Summary.
    #[command(subcommand)]
    Summary(SummaryCommand),
    /// The Voice Registry: every Speaker and Voiceprint this app holds.
    #[command(subcommand)]
    Speakers(SpeakerCommand),
    /// Re-assign a Transcript segment to a different Speaker. Your
    /// correction layers above the machine's attribution and never
    /// rewrites it.
    Reassign {
        /// Segment id, from `evertranscript show <meeting> --json`.
        segment: String,
        /// Speaker id, from `evertranscript speakers list`.
        speaker: String,
    },
    /// Run Diarization over a Meeting, see what it is doing, or stop it.
    #[command(subcommand)]
    Diarize(DiarizeCommand),
    /// Turn launch-at-login on or off. Registration only: a running Core is
    /// left alone, and Quit is what stops it.
    Autostart {
        #[arg(value_parser = ["on", "off"])]
        state: String,
    },
}

#[derive(Subcommand)]
enum NotesCommand {
    /// Print a Meeting's Notes.
    Show { meeting: String },
    /// Replace them. Reads from stdin when no text is given, so an editor
    /// or a pipe works.
    Set {
        meeting: String,
        text: Option<String>,
    },
}

#[derive(Subcommand)]
enum SummaryCommand {
    /// Print a Meeting's Summary, if it has one.
    Show { meeting: String },
    /// Generate one on the chosen Backend.
    Generate { meeting: String },
    /// Show the Summary Backends and which one is chosen.
    Backends {
        #[arg(long)]
        json: bool,
    },
    /// Choose a Backend. `local` is the bundled model; anything else sends
    /// meeting content to that provider.
    Use {
        /// `local`, a preset id, or a custom id used with --base-url.
        backend: String,
        #[arg(long)]
        base_url: Option<String>,
        /// Required to choose anything but `local`: confirms you have read
        /// what leaves this machine.
        #[arg(long)]
        i_understand_this_sends_my_meetings: bool,
    },
    /// Never auto-switch Backends; report the failure instead (story 39).
    Strict { state: String },
    /// Store an API key in the OS credential store. Reads from stdin so it
    /// never lands in your shell history.
    SetKey { provider: String },
    /// Forget a stored API key.
    ClearKey { provider: String },
    /// Show the system prompt, or replace it. `--reset` restores the default.
    Prompt {
        text: Option<String>,
        #[arg(long)]
        reset: bool,
    },
}

#[derive(Subcommand)]
enum SpeakerCommand {
    /// Every Speaker this app holds, and whether it can still recognize
    /// each voice.
    List {
        #[arg(long)]
        json: bool,
    },
    /// One Speaker, with where it has been heard and what the calendar
    /// suggests calling it.
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Name a Speaker. Every past appearance is relabelled, and the name
    /// also confirms the Voiceprint for future matching.
    Rename { id: String, name: String },
    /// Delete a Speaker's Voiceprint. The app stops recognizing that voice;
    /// nothing in the record changes.
    Forget {
        id: String,
        /// Skip the confirmation prompt.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum DiarizeCommand {
    /// Whether a Meeting is being diarized right now.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Diarize a finished Meeting, or re-diarize one after a model upgrade.
    /// Your corrections survive a re-run.
    Run { meeting: String },
    /// Stop a running Diarization. Whatever attribution finished is kept.
    Cancel { meeting: String },
}

#[derive(Subcommand)]
enum WatchlistCommand {
    /// Show what is watched, and what is offered.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Watch an app. Membership is the switch: there is nothing to enable
    /// afterwards.
    Add {
        /// Bundle id on macOS, executable name on Windows.
        id: String,
        /// What to call it. Defaults to the id, or to the suggested entry's
        /// own name when promoting one.
        #[arg(long)]
        name: Option<String>,
    },
    /// Stop watching an app.
    Remove { id: String },
}

#[derive(Subcommand)]
enum ModelsCommand {
    /// Show what is on disk and what is still needed.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Download missing models, resuming any partial download.
    Fetch {
        /// One model by key; omit to fetch everything required.
        key: Option<String>,
    },
}

#[derive(Subcommand)]
enum RecordCommand {
    /// Begin recording a Meeting.
    Start {
        #[arg(long)]
        title: Option<String>,
        /// The app to attribute this Meeting to (the Mirror's slug until it
        /// has a title).
        #[arg(long)]
        app: Option<String>,
    },
    /// Stop the Meeting in progress and persist it.
    Stop,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    // The daemon is the one command that does not simply block on a future.
    // A menu bar item has to own the main thread (ADR-0023), so the Core
    // runs on the runtime's threads and this one goes to the platform.
    if matches!(cli.command, Command::Daemon) {
        return run_daemon_owning_the_main_thread(runtime);
    }
    runtime.block_on(run(cli))
}

/// Starts the Core, then gives the main thread to the tray.
///
/// If there is no tray to run — a headless machine, a CI runner, or
/// `EVERTRANSCRIPT_NO_TRAY` — this blocks on the Core instead and behaves
/// exactly as it did before the tray existed. That fallback is the point:
/// the menu bar is an addition to the daemon, never a requirement of it.
fn run_daemon_owning_the_main_thread(runtime: tokio::runtime::Runtime) -> Result<()> {
    use evertranscript_core::tray;

    init_tracing();
    let shutdown = CancellationToken::new();
    let signal_shutdown = shutdown.clone();
    runtime.spawn(async move {
        if let Err(err) = wait_for_shutdown_signal().await {
            tracing::warn!(%err, "signal handling failed; the Core will run until killed");
            return;
        }
        tracing::info!("shutdown signal received");
        signal_shutdown.cancel();
    });

    let daemon = runtime.block_on(evertranscript_core::start_daemon(shutdown.clone()))?;

    // **The binary asks; construction does not.** A fresh install fetches what
    // it needs so the Operator finds the features there rather than finding a
    // download — but a Core that is merely built provisions nothing, which is
    // what lets the guarantee tests keep proving that a full record-and-
    // summarize cycle opens no socket at all.
    //
    // Detached and never fatal: recording must work while this runs, and a
    // fetch that fails is a retry rather than a startup error (ADR-0019).
    {
        let core = Arc::clone(daemon.core());
        let cancel = shutdown.clone();
        runtime.spawn(async move {
            // Preselect before provisioning: the two describe the same fresh
            // install, and an Operator who opens Settings while the download
            // runs should already see a Backend chosen.
            // Before provisioning, so an upgrade reclaims the old model's
            // space rather than holding both at once.
            core.remove_superseded_models();
            if let Err(error) = core.preselect_local_backend().await {
                tracing::warn!(%error, "could not record the preselected Backend");
            }
            if let Err(error) = core.provision_if_fresh(cancel).await {
                tracing::warn!(%error, "provisioning did not complete; models can be fetched later");
            }
        });
    }

    let controller = tray::TrayController::new(
        Arc::clone(daemon.core()),
        runtime.handle().clone(),
        shutdown.clone(),
    );
    runtime.spawn(tray::poll(Arc::clone(&controller)));

    match tray::run(controller) {
        // The tray's run loop returning means the Operator chose Quit, or
        // shutdown was signalled elsewhere. Either way the Core is done.
        Ok(()) => shutdown.cancel(),
        // No tray on this machine. Serve until a signal says otherwise —
        // exactly what the daemon did before the menu bar existed. Ending
        // here instead would turn every headless Core into one that starts
        // and immediately exits.
        Err(reason) => tracing::info!(%reason, "running without a menu bar item"),
    }
    runtime.block_on(daemon.join());
    Ok(())
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        // Handled in main(): the daemon needs the main thread for the tray,
        // so it never reaches this dispatch.
        Command::Daemon => unreachable!("the daemon is started before this point"),
        Command::Status { json } => run_status(json).await,
        Command::Paths { json } => {
            print_paths(json);
            Ok(())
        }
        Command::Record(RecordCommand::Start { title, app }) => run_record_start(title, app).await,
        Command::Record(RecordCommand::Stop) => run_record_stop().await,
        Command::List { limit, json } => run_list(limit, json).await,
        Command::Show { id, json } => run_show(&id, json).await,
        Command::Retitle { id, title } => run_retitle(&id, &title).await,
        Command::Delete { id, force } => run_delete(&id, force).await,
        Command::Search { query, limit, json } => run_search(&query.join(" "), limit, json).await,
        Command::Export { id } => run_export(&id).await,
        Command::Models(ModelsCommand::Status { json }) => run_models_status(json).await,
        Command::Models(ModelsCommand::Fetch { key }) => run_models_fetch(key).await,
        Command::Watchlist(watchlist) => run_watchlist(watchlist).await,
        Command::Captions => run_captions().await,
        Command::Settings {
            json,
            chinese_script,
        } => run_settings(json, chinese_script).await,
        Command::Acknowledge => run_acknowledge().await,
        Command::AudioCheck { seconds } => run_audio_check(seconds).await,
        Command::Notes(notes) => run_notes(notes).await,
        Command::Summary(summary) => run_summary(summary).await,
        Command::Speakers(speakers) => run_speakers(speakers).await,
        Command::Reassign { segment, speaker } => run_reassign(&segment, &speaker).await,
        Command::Diarize(diarize) => run_diarize(diarize).await,
        Command::Autostart { state } => run_autostart(&state).await,
    }
}

/// Captures for a few seconds and reports what each leg actually produced.
///
/// The preflight deliberately *records* instead of asking the OS whether it
/// may. On macOS the two answers differ: a tap is granted whether or not the
/// Operator has allowed audio recording, and a refused one delivers silence
/// forever without ever failing. Asking would report a working system-audio
/// leg on a machine that will record nothing — so this listens instead, and
/// reports what arrived.
async fn run_audio_check(seconds: u64) -> Result<()> {
    use evertranscript_protocol::AudioChannel;
    use evertranscript_protocol::AudioCheckVerdict;
    use evertranscript_protocol::AudioLegState;

    println!("Listening for {seconds}s. Play some audio — a meeting, a video, anything.\n");
    // In-process on purpose: this subcommand is documented to run without the
    // Core, so it works before anything is installed. The Client asks the
    // Core for the same check over `audio/check`, and both land in the same
    // function — the two surfaces cannot come to different conclusions about
    // the same machine.
    let report = evertranscript_core::audio::check::run(seconds).await;

    if let Some(error) = &report.could_not_start {
        println!("Nothing can be recorded on this machine:\n  {error}");
    }

    for leg in &report.legs {
        let name = match leg.channel {
            AudioChannel::Mic => "Microphone  ",
            AudioChannel::System => "System audio",
        };
        let ms = leg.milliseconds;
        match leg.state {
            AudioLegState::Working => {
                println!("{name}  {ms} ms captured, peak level {:.3}", leg.peak)
            }
            AudioLegState::NotTested => {
                println!("{name}  {ms} ms captured, but nothing was playing");
                println!("              play some audio and run this again to check this leg");
            }
            AudioLegState::Silent => println!("{name}  {ms} ms captured, but all of it silent"),
            AudioLegState::NothingCaptured => println!("{name}  nothing captured"),
        }
        if let Some(reason) = &leg.reason {
            println!("              {reason}");
        }
    }

    println!();
    println!(
        "{}",
        match report.verdict {
            AudioCheckVerdict::BothLegsWork => "Both legs work. Meetings will record in full.",
            AudioCheckVerdict::MicrophoneWorksOtherUntested =>
                "The microphone works. The other leg was not tested — play some audio and run this again.",
            AudioCheckVerdict::OneLegWorks =>
                "One leg works. Meetings will record, and be marked partial.",
            AudioCheckVerdict::NothingCaptured =>
                "No audio was captured. Meetings would record nothing.",
            AudioCheckVerdict::NothingTested =>
                "Nothing could be tested — play some audio and run this again.",
        }
    );
    Ok(())
}

async fn run_settings(json: bool, chinese_script: Option<String>) -> Result<()> {
    let mut client = client().await?;
    let settings: SettingsResponse = match chinese_script {
        Some(choice) => {
            let script = match choice.as_str() {
                "traditional" => ChineseScript::Traditional,
                _ => ChineseScript::Simplified,
            };
            client
                .request(
                    "settings/set",
                    Some(serde_json::json!({ "chineseScript": script })),
                )
                .await?
        }
        None => client.request("settings/get", None).await?,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&settings)?);
        return Ok(());
    }
    println!(
        "briefing acknowledged  {}",
        if settings.briefing_acknowledged {
            "yes"
        } else {
            "no — nothing will be recorded"
        }
    );
    println!(
        "auto-record            {}",
        if settings.auto_record { "on" } else { "off" }
    );
    println!(
        "chinese script         {}",
        match settings.chinese_script {
            ChineseScript::Simplified => "simplified",
            ChineseScript::Traditional => "traditional",
        }
    );
    println!(
        "launch at login        {}",
        if settings.launch_at_login {
            "on"
        } else {
            "off"
        }
    );
    println!(
        "  registration         {}",
        settings.launch_at_login_location
    );
    if settings.launch_at_login != settings.launch_at_login_registered {
        println!(
            "  note                 the setting and the actual registration disagree; \
             run `evertranscript autostart {}` to reconcile",
            if settings.launch_at_login {
                "on"
            } else {
                "off"
            }
        );
    }
    Ok(())
}

async fn run_acknowledge() -> Result<()> {
    // The whole Briefing, printed. Acknowledging something the Operator was
    // never shown is the failure this command exists to avoid, and a summary
    // of a legal notice is not the notice.
    let mut reader = client().await?;
    let briefing: BriefingResponse = reader.request("briefing/get", None).await?;
    println!("{}\n", briefing.text);
    if briefing.acknowledged {
        println!("(already acknowledged on this machine)");
        return Ok(());
    }
    if briefing.awaiting_counsel {
        println!("---\n");
    }

    // A pause between the text and the act, so acknowledgment follows
    // reading rather than accompanying it. Only when someone is actually
    // there: a script or a test invoking this has already made the
    // deliberate choice that a prompt exists to elicit, and blocking on a
    // pipe that will never answer would hang instead of asking.
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        eprint!("Acknowledge this and allow recording on this machine? [y/N] ");
        use std::io::Write;
        std::io::stderr().flush().ok();
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim(), "y" | "Y" | "yes") {
            println!("Not acknowledged. Nothing will be captured.");
            return Ok(());
        }
    }

    let mut client = client().await?;
    let settings: SettingsResponse = client
        .request(
            "settings/set",
            Some(serde_json::json!({ "briefingAcknowledged": true })),
        )
        .await?;
    println!(
        "\nacknowledged — recording is now permitted ({}).",
        if settings.briefing_acknowledged {
            "confirmed"
        } else {
            "not saved"
        }
    );
    Ok(())
}

async fn run_autostart(state: &str) -> Result<()> {
    let enabled = state == "on";
    let mut client = client().await?;
    let settings: SettingsResponse = client
        .request(
            "settings/set",
            Some(serde_json::json!({ "launchAtLogin": enabled })),
        )
        .await?;
    println!(
        "launch at login {} ({})",
        if settings.launch_at_login {
            "on"
        } else {
            "off"
        },
        settings.launch_at_login_location
    );
    if !enabled {
        println!("the running Core is untouched; use the tray's Quit to stop it now");
    }
    Ok(())
}

async fn run_captions() -> Result<()> {
    let mut client = client().await?;
    // Snapshot and subscription in one call, so nothing said between them is
    // missed (ADR-0028).
    let snapshot: TranscriptSnapshotResponse = client.request("transcript/subscribe", None).await?;

    match &snapshot.meeting {
        Some(meeting) => println!(
            "following {} — Ctrl-C to stop\n",
            meeting
                .title
                .clone()
                .unwrap_or_else(|| format!("meeting {}", &meeting.id[..8]))
        ),
        None => println!("nothing is recording; waiting for a Meeting to start\n"),
    }
    for segment in &snapshot.segments {
        print_caption(segment);
    }

    while let Some(notification) = client.next_notification().await? {
        match notification.method.as_str() {
            "transcript/segmentAdded" => {
                if let Some(segment) = notification
                    .params
                    .as_ref()
                    .and_then(|params| params.get("segment"))
                    .and_then(|segment| {
                        serde_json::from_value::<evertranscript_protocol::TranscriptSegment>(
                            segment.clone(),
                        )
                        .ok()
                    })
                {
                    print_caption(&segment);
                }
            }
            "transcript/captionsDropped" => {
                let dropped = notification
                    .params
                    .as_ref()
                    .and_then(|params| params.get("dropped"))
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0);
                eprintln!("… {dropped} captions dropped (this terminal fell behind)");
            }
            _ => {}
        }
    }
    Ok(())
}

fn print_caption(segment: &evertranscript_protocol::TranscriptSegment) {
    let speaker = match segment.channel {
        evertranscript_protocol::AudioChannel::Mic => "You",
        evertranscript_protocol::AudioChannel::System => "Participants",
    };
    let seconds = segment.start_ms / 1000;
    println!(
        "[{:02}:{:02}] {speaker}: {}",
        seconds / 60,
        seconds % 60,
        segment.text
    );
}

async fn run_models_status(json: bool) -> Result<()> {
    let mut client = client().await?;
    let response: ModelsStatusResponse = client.request("models/status", None).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(());
    }
    for model in &response.models {
        let state = match model.state {
            ModelAvailability::Ready => "ready".to_string(),
            ModelAvailability::Missing => "missing".to_string(),
            ModelAvailability::Partial => match model.bytes_on_disk {
                Some(bytes) => format!(
                    "partial ({}%)",
                    (bytes as f64 / model.total_bytes.max(1) as f64 * 100.0) as u32
                ),
                None => "partial".to_string(),
            },
            ModelAvailability::Corrupted => "corrupted".to_string(),
        };
        println!("{:<32} {:<16} {}", model.key, state, model.display_name);
        if let Some(detail) = &model.detail {
            println!("{:<32} {detail}", "");
        }
    }
    if !response.ready {
        println!("\nrun `evertranscript models fetch` to download what is missing");
    }
    Ok(())
}

async fn run_models_fetch(key: Option<String>) -> Result<()> {
    let mut client = client().await?;
    println!(
        "downloading… (this can take a while on a slow link; interrupting is safe — it resumes)"
    );
    let response: ModelsStatusResponse = client
        .request("models/fetch", Some(serde_json::json!({ "key": key })))
        .await?;
    for model in &response.models {
        if model.state == ModelAvailability::Ready {
            println!("ready  {}", model.key);
        }
    }
    Ok(())
}

async fn client() -> Result<CoreClient> {
    CoreClient::connect_initialized("evertranscript-cli").await
}

async fn run_record_start(title: Option<String>, app: Option<String>) -> Result<()> {
    let mut client = client().await?;
    let response: MeetingResponse = client
        .request(
            "meeting/start",
            Some(serde_json::json!({ "title": title, "detectedApp": app })),
        )
        .await?;
    println!("recording {}", response.meeting.id);
    Ok(())
}

async fn run_record_stop() -> Result<()> {
    let mut client = client().await?;
    let response: MeetingResponse = client.request("meeting/stop", None).await?;
    let meeting = response.meeting;
    println!(
        "stopped {} ({})",
        meeting.id,
        meeting
            .duration_seconds
            .map(format_uptime)
            .unwrap_or_else(|| "unknown length".to_string())
    );
    if let Some(filename) = meeting.mirror_filename {
        println!("mirror  {filename}");
    }
    Ok(())
}

async fn run_list(limit: u32, json: bool) -> Result<()> {
    let mut client = client().await?;
    let response: MeetingListResponse = client
        .request("meeting/list", Some(serde_json::json!({ "limit": limit })))
        .await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&response.meetings)?);
        return Ok(());
    }
    if response.meetings.is_empty() {
        println!("no Meetings yet");
        return Ok(());
    }
    for meeting in response.meetings {
        println!(
            "{}  {}  {}",
            &meeting.id[..8],
            meeting.started_at,
            display_title(&meeting)
        );
    }
    Ok(())
}

async fn run_show(id: &str, json: bool) -> Result<()> {
    let mut client = client().await?;
    let response: MeetingDetailResponse = client
        .request("meeting/get", Some(serde_json::json!({ "id": id })))
        .await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(());
    }
    println!("{}", display_title(&response.meeting));
    println!("  id       {}", response.meeting.id);
    println!("  started  {}", response.meeting.started_at);
    if let Some(filename) = &response.meeting.mirror_filename {
        println!("  mirror   {filename}");
    }
    // Before the transcript, because the transcript is only as complete as
    // the capture was.
    if !response.meeting.audio_notes.is_empty() {
        println!("\nthis recording is incomplete:");
        for note in &response.meeting.audio_notes {
            println!("  - {note}");
        }
    }
    if response.segments.is_empty() {
        println!("\nno transcript yet");
    } else {
        println!();
        for segment in response.segments {
            println!("[{}] {}", segment.start_ms / 1000, segment.text);
        }
    }
    Ok(())
}

async fn run_retitle(id: &str, title: &str) -> Result<()> {
    let mut client = client().await?;
    let response: MeetingResponse = client
        .request(
            "meeting/retitle",
            Some(serde_json::json!({ "id": id, "title": title })),
        )
        .await?;
    println!(
        "renamed to {}",
        response
            .meeting
            .mirror_filename
            .unwrap_or_else(|| title.to_string())
    );
    Ok(())
}

async fn run_delete(id: &str, force: bool) -> Result<()> {
    // Deleting a Meeting removes its audio too and cannot be undone, so the
    // CLI asks unless told not to.
    if !force {
        eprint!("delete Meeting {id} — transcript, Mirror, and audio? [y/N] ");
        use std::io::Write;
        std::io::stderr().flush().ok();
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim(), "y" | "Y" | "yes") {
            println!("left alone");
            return Ok(());
        }
    }
    let mut client = client().await?;
    let response: MeetingDeleteResponse = client
        .request("meeting/delete", Some(serde_json::json!({ "id": id })))
        .await?;
    println!(
        "{}",
        if response.deleted {
            "deleted"
        } else {
            "no such Meeting"
        }
    );
    Ok(())
}

async fn run_search(query: &str, limit: u32, json: bool) -> Result<()> {
    let mut client = client().await?;
    let response: HistorySearchResponse = client
        .request(
            "history/search",
            Some(serde_json::json!({ "query": query, "limit": limit })),
        )
        .await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&response.results)?);
        return Ok(());
    }
    if response.results.is_empty() {
        println!("nothing matched");
        return Ok(());
    }
    for result in response.results {
        println!(
            "{}  {}",
            &result.meeting.id[..8],
            display_title(&result.meeting)
        );
        let snippet = result.snippet.replace('\n', " ");
        if !snippet.trim().is_empty() {
            println!("    {}", snippet.trim());
        }
    }
    Ok(())
}

async fn run_export(id: &str) -> Result<()> {
    let mut client = client().await?;
    let response: MeetingExportResponse = client
        .request("meeting/export", Some(serde_json::json!({ "id": id })))
        .await?;
    print!("{}", response.markdown);
    Ok(())
}

fn display_title(meeting: &Meeting) -> String {
    meeting
        .title
        .clone()
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| match &meeting.detected_app {
            Some(app) => format!("({app}, untitled)"),
            None => "(untitled)".to_string(),
        })
}

async fn wait_for_shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::SignalKind;
        use tokio::signal::unix::signal;
        let mut terminate = signal(SignalKind::terminate())?;
        let mut interrupt = signal(SignalKind::interrupt())?;
        tokio::select! {
            _ = terminate.recv() => {}
            _ = interrupt.recv() => {}
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await?;
        Ok(())
    }
}

async fn run_status(json: bool) -> Result<()> {
    let mut client = CoreClient::connect_initialized("evertranscript-cli").await?;
    let status = client.status().await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }
    println!("EverTranscript Core {}", status.version);
    println!("  pid       {}", status.pid);
    println!("  uptime    {}", format_uptime(status.uptime_seconds));
    println!("  state     {:?}", status.state);
    println!("  history   {}", status.history_dir);
    if let Some(warning) = status.incomplete_copy_warning {
        println!("\nwarning: {warning}");
    }
    Ok(())
}

fn print_paths(json: bool) {
    let entries = [
        ("history", paths::history_dir().display().to_string()),
        ("data", paths::data_dir().display().to_string()),
        ("audio", paths::audio_dir().display().to_string()),
        ("database", paths::database_path().display().to_string()),
        ("models", paths::models_dir().display().to_string()),
        ("listen", paths::listen_address_display()),
    ];
    if json {
        let map: serde_json::Map<String, serde_json::Value> = entries
            .iter()
            .map(|(key, value)| ((*key).to_string(), serde_json::Value::String(value.clone())))
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::Value::Object(map))
                .unwrap_or_else(|_| "{}".to_string())
        );
        return;
    }
    for (key, value) in entries {
        println!("{key:<9} {value}");
    }
}

fn format_uptime(seconds: u64) -> String {
    let (hours, minutes, seconds) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    if hours > 0 {
        format!("{hours}h {minutes}m {seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_env("EVERTRANSCRIPT_LOG")
        .unwrap_or_else(|_| EnvFilter::new("evertranscript=info,evertranscript_core=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

/// Notes: the Operator's own writing (ADR-0018).
async fn run_notes(command: NotesCommand) -> Result<()> {
    let mut client = client().await?;
    match command {
        NotesCommand::Show { ref meeting } => {
            let response: MeetingDetailResponse = client
                .request("meeting/get", Some(serde_json::json!({ "id": meeting })))
                .await?;
            match response.meeting.notes.as_deref() {
                Some(notes) => println!("{notes}"),
                None => println!("(no notes)"),
            }
        }
        NotesCommand::Set {
            ref meeting,
            ref text,
        } => {
            // Reading stdin when no text is given is what makes an editor or
            // a pipe work, which is how anyone actually writes more than a
            // sentence.
            let notes = match text {
                Some(text) => text.clone(),
                None => {
                    use std::io::Read;
                    let mut buffer = String::new();
                    std::io::stdin().read_to_string(&mut buffer)?;
                    buffer
                }
            };
            let response: MeetingResponse = client
                .request(
                    "meeting/setNotes",
                    Some(serde_json::json!({ "id": meeting, "notes": notes })),
                )
                .await?;
            println!(
                "Saved {} character(s) of notes.",
                response
                    .meeting
                    .notes
                    .as_deref()
                    .unwrap_or("")
                    .chars()
                    .count()
            );
        }
    }
    Ok(())
}

async fn run_summary(command: SummaryCommand) -> Result<()> {
    let mut client = client().await?;
    match command {
        SummaryCommand::Show { ref meeting } => {
            let response: MeetingDetailResponse = client
                .request("meeting/get", Some(serde_json::json!({ "id": meeting })))
                .await?;
            match response.meeting.summary.as_deref() {
                Some(summary) => {
                    println!("{summary}");
                    if let Some(backend) = response.meeting.summary_backend.as_deref() {
                        // Which Backend produced it, always. An Operator who
                        // chose Cloud and received local quality is owed the
                        // reason; one who chose Local is owed the evidence.
                        println!("\n---\ngenerated by {backend}");
                    }
                }
                None => println!("(no summary yet — `evertranscript summary generate {meeting}`)"),
            }
        }
        SummaryCommand::Generate { ref meeting } => {
            println!("Generating… this can take a few minutes on a local model.");
            let response: MeetingResponse = client
                .request(
                    "summary/generate",
                    Some(serde_json::json!({ "id": meeting })),
                )
                .await?;
            println!(
                "{}",
                response
                    .meeting
                    .summary
                    .as_deref()
                    .unwrap_or("(nothing generated)")
            );
        }
        SummaryCommand::Backends { json } => {
            let response: SummaryBackendsResponse =
                client.request("summary/backends", None).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&response)?);
                return Ok(());
            }
            match response.chosen.as_deref() {
                // ADR-0013: unchosen is a state to show, not to default away.
                None => println!("No Summary Backend chosen yet. Nothing will be generated.\n"),
                Some(chosen) => println!("Using: {chosen}\n"),
            }
            for option in &response.options {
                let mark = if response.chosen.as_deref() == Some(option.id.as_str()) {
                    "*"
                } else {
                    " "
                };
                let where_it_goes = if option.leaves_the_machine {
                    "sends your meetings to this provider"
                } else {
                    "stays on this machine"
                };
                println!("{mark} {:<12} {where_it_goes}", option.id);
                if let Some(handling) = &option.data_handling {
                    println!(
                        "    trains on inputs: {} · retention: {} · zero-retention: {} · \
                         verified: {}",
                        handling.trains_on_inputs,
                        handling.retention,
                        handling.zero_retention_available,
                        handling.verified_on
                    );
                }
                if option.leaves_the_machine {
                    println!(
                        "    api key stored: {}",
                        if option.has_key { "yes" } else { "no" }
                    );
                }
            }
            println!(
                "\nAny other id with --base-url is accepted: {}",
                response.custom_endpoint_label
            );
            if response.strict {
                println!("Strict Mode is on: a failing Backend is reported, never switched.");
            }
        }
        SummaryCommand::Use {
            ref backend,
            ref base_url,
            i_understand_this_sends_my_meetings,
        } => {
            let cloud = backend != "local";
            if cloud && !i_understand_this_sends_my_meetings {
                // The hard one-time warning (story 36), in the surface where
                // the choice is actually made.
                println!(
                    "Choosing `{backend}` sends the full text of your meetings to that \
                     provider.\n\nEverything else in EverTranscript stays on this machine: \
                     recording, transcription, and speaker attribution are permanently local \
                     and have no cloud option at all. Summary is the single exception, and \
                     this is it.\n\nRe-run with \
                     --i-understand-this-sends-my-meetings to proceed."
                );
                return Ok(());
            }
            let mut change = serde_json::Map::new();
            if cloud {
                change.insert("summaryCloudWarningAccepted".into(), true.into());
            }
            change.insert("summaryBackend".into(), backend.clone().into());
            if let Some(url) = base_url {
                change.insert("summaryBaseUrl".into(), url.clone().into());
            }
            let _: SettingsResponse = client
                .request("settings/set", Some(serde_json::Value::Object(change)))
                .await?;
            println!("Summary Backend is now {backend}.");
        }
        SummaryCommand::Strict { ref state } => {
            let on = matches!(state.as_str(), "on" | "true" | "yes");
            let _: SettingsResponse = client
                .request(
                    "settings/set",
                    Some(serde_json::json!({ "summaryStrict": on })),
                )
                .await?;
            println!(
                "Strict Mode {}. {}",
                if on { "on" } else { "off" },
                if on {
                    "A failing Backend is reported, never switched."
                } else {
                    "A failing cloud Backend falls back to local. Local never falls back."
                }
            );
        }
        SummaryCommand::SetKey { ref provider } => {
            // From stdin, so the key never reaches shell history or `ps`.
            use std::io::Read;
            let mut key = String::new();
            std::io::stdin().read_to_string(&mut key)?;
            let _: SummaryBackendsResponse = client
                .request(
                    "summary/setKey",
                    Some(serde_json::json!({ "provider": provider, "key": key.trim() })),
                )
                .await?;
            println!("Stored in the OS credential store. It is never read back out.");
        }
        SummaryCommand::ClearKey { ref provider } => {
            let _: SummaryBackendsResponse = client
                .request(
                    "summary/setKey",
                    Some(serde_json::json!({ "provider": provider })),
                )
                .await?;
            println!("Forgot the key for {provider}.");
        }
        SummaryCommand::Prompt { ref text, reset } => {
            if reset {
                let _: SettingsResponse = client
                    .request(
                        "settings/set",
                        Some(serde_json::json!({ "summaryPrompt": "" })),
                    )
                    .await?;
                println!("Reset to the default.");
                return Ok(());
            }
            match text {
                Some(text) => {
                    let _: SettingsResponse = client
                        .request(
                            "settings/set",
                            Some(serde_json::json!({ "summaryPrompt": text })),
                        )
                        .await?;
                    println!("Saved.");
                }
                None => {
                    let response: SettingsResponse = client.request("settings/get", None).await?;
                    println!(
                        "{}",
                        response
                            .summary_prompt
                            .as_deref()
                            .unwrap_or(&response.summary_prompt_default)
                    );
                }
            }
        }
    }
    Ok(())
}

/// The Voice Registry (ADR-0008's mandatory legibility surface).
///
/// It answers "what does this app know about voices?" without a Meeting
/// open, because the inventory is a property of the installation and not of
/// any one recording.
async fn run_speakers(command: SpeakerCommand) -> Result<()> {
    let mut client = client().await?;
    match command {
        SpeakerCommand::List { json } => {
            let response: SpeakerListResponse = client.request("speaker/list", None).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&response)?);
                return Ok(());
            }
            if response.speakers.is_empty() {
                println!(
                    "No Speakers yet. They are created by Diarization, not by enrolling anyone."
                );
                return Ok(());
            }
            println!(
                "{:<38}  {:<20}  {:>8}  VOICEPRINT",
                "ID", "NAME", "MEETINGS"
            );
            for speaker in &response.speakers {
                println!(
                    "{:<38}  {:<20}  {:>8}  {}",
                    speaker.id,
                    display_name_of(speaker),
                    speaker.meetings_seen_in,
                    voiceprint_state(speaker),
                );
            }
            println!(
                "\n{} Speaker(s); {} with a stored Voiceprint.",
                response.speakers.len(),
                response
                    .speakers
                    .iter()
                    .filter(|speaker| speaker.has_voiceprint)
                    .count()
            );
        }
        SpeakerCommand::Show { ref id, json } => {
            let response: SpeakerDetailResponse = client
                .request("speaker/get", Some(serde_json::json!({ "id": id })))
                .await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&response)?);
                return Ok(());
            }
            let speaker = &response.speaker;
            println!("{}", display_name_of(speaker));
            println!("  id          {}", speaker.id);
            println!("  voiceprint  {}", voiceprint_state(speaker));
            println!("  meetings    {}", speaker.meetings_seen_in);
            if let Some(first) = &speaker.first_seen_at {
                println!("  first seen  {first}");
            }
            if let Some(model) = &speaker.voiceprint_model {
                println!("  model       {model}");
            }
            if !response.name_suggestions.is_empty() {
                println!(
                    "\n  The calendar listed these people in meetings this voice was in.\n  \
                     Suggestions only — being invited is not evidence of having spoken:"
                );
                for name in &response.name_suggestions {
                    println!("    {name}");
                }
            }
        }
        SpeakerCommand::Rename { ref id, ref name } => {
            let response: SpeakerResponse = client
                .request(
                    "speaker/rename",
                    Some(serde_json::json!({ "id": id, "displayName": name })),
                )
                .await?;
            println!(
                "Renamed to {}. Every past appearance now reads that way, and the Voiceprint is \
                 confirmed for future matching.",
                display_name_of(&response.speaker)
            );
        }
        SpeakerCommand::Forget { ref id, force } => {
            // Said before it happens rather than after, because the whole
            // point of this surface is that a biometric deletion is a
            // legible act (ADR-0008, ADR-0009).
            if !force {
                println!(
                    "Deleting this Voiceprint stops the app recognizing that voice in future \
                     Meetings.\nNothing in the record changes: the Speaker, its name, and every \
                     word attributed to it stay exactly as they are.\n"
                );
                eprint!("delete this Voiceprint? [y/N] ");
                use std::io::Write;
                std::io::stderr().flush().ok();
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)?;
                if !matches!(answer.trim(), "y" | "Y" | "yes") {
                    println!("left alone");
                    return Ok(());
                }
            }
            let response: SpeakerResponse = client
                .request(
                    "speaker/deleteVoiceprint",
                    Some(serde_json::json!({ "id": id })),
                )
                .await?;
            println!(
                "Forgot the voice of {}. The record is unchanged.",
                display_name_of(&response.speaker)
            );
        }
    }
    Ok(())
}

fn display_name_of(speaker: &evertranscript_protocol::Speaker) -> String {
    match (&speaker.display_name, speaker.is_operator) {
        (Some(name), _) => name.clone(),
        (None, true) => "You".to_string(),
        // Deliberately not a stored pseudonym: a persisted "Speaker 3" would
        // read as a name somebody chose.
        (None, false) => format!("(unnamed {})", &speaker.id[..8]),
    }
}

fn voiceprint_state(speaker: &evertranscript_protocol::Speaker) -> &'static str {
    match (speaker.has_voiceprint, speaker.confirmed) {
        (false, _) => "none",
        (true, true) => "confirmed",
        (true, false) => "unconfirmed",
    }
}

/// Re-assigns one segment (story 29b).
async fn run_reassign(segment: &str, speaker: &str) -> Result<()> {
    let mut client = client().await?;
    let response: TranscriptReassignResponse = client
        .request(
            "transcript/reassign",
            Some(serde_json::json!({ "segmentId": segment, "speakerId": speaker })),
        )
        .await?;
    println!(
        "Re-assigned. Your correction sits above the machine's attribution, which is kept \
         underneath.\n  {}",
        response.segment.text.trim()
    );
    Ok(())
}

async fn run_diarize(command: DiarizeCommand) -> Result<()> {
    let mut client = client().await?;
    let response: DiarizeStatusResponse = match command {
        DiarizeCommand::Status { .. } => client.request("diarize/status", None).await?,
        DiarizeCommand::Run { ref meeting } => {
            let started: DiarizeStatusResponse = client
                .request(
                    "diarize/run",
                    Some(serde_json::json!({ "meetingId": meeting })),
                )
                .await?;
            println!(
                "Diarizing in the background. Follow it with `evertranscript diarize status`."
            );
            started
        }
        DiarizeCommand::Cancel { ref meeting } => {
            client
                .request(
                    "diarize/cancel",
                    Some(serde_json::json!({ "meetingId": meeting })),
                )
                .await?
        }
    };
    if matches!(command, DiarizeCommand::Status { json: true }) {
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(());
    }
    match response.state {
        DiarizeState::Idle => println!("Nothing is being diarized."),
        DiarizeState::Unavailable => {
            println!("Diarization is unavailable — its models are missing or unreadable.")
        }
        DiarizeState::Running => {
            let percent = if response.total_ms > 0 {
                response.done_ms * 100 / response.total_ms
            } else {
                0
            };
            println!(
                "Diarizing {} — {percent}%",
                response.meeting_id.as_deref().unwrap_or("a Meeting")
            );
        }
    }
    Ok(())
}

async fn run_watchlist(command: WatchlistCommand) -> Result<()> {
    let mut client = client().await?;
    let response: WatchlistResponse = match command {
        WatchlistCommand::List { .. } => client.request("watchlist/get", None).await?,
        WatchlistCommand::Add { ref id, ref name } => {
            client
                .request(
                    "watchlist/add",
                    Some(serde_json::json!({ "id": id, "name": name })),
                )
                .await?
        }
        WatchlistCommand::Remove { ref id } => {
            client
                .request("watchlist/remove", Some(serde_json::json!({ "id": id })))
                .await?
        }
    };

    if matches!(command, WatchlistCommand::List { json: true }) {
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(());
    }

    if response.entries.is_empty() {
        println!("Nothing is watched. Meeting Detection will not start a recording.");
    }
    for entry in &response.entries {
        let kind = match entry.kind {
            WatchlistKind::BrowserMeetings => "any browser in a call",
            WatchlistKind::Process => entry.id.as_str(),
        };
        println!("  {:<20} {kind}", entry.name);
    }
    if !response.suggestions.is_empty() {
        println!("\nSuggested — add with `evertranscript watchlist add <id>`:");
        for entry in &response.suggestions {
            println!("  {:<20} {}", entry.name, entry.id);
        }
    }
    Ok(())
}
