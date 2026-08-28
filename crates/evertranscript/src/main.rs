//! EverTranscript: one binary that is both the Core daemon and the CLI.
//!
//! `evertranscript daemon` runs the Core — the login item that detects,
//! captures, transcribes, and stores. Every other subcommand is a Client of
//! a running Core over the local protocol (ADR-0026): the CLI never touches
//! the record directly.

use anyhow::Result;
use clap::Parser;
use clap::Subcommand;
use evertranscript_core::client::CoreClient;
use evertranscript_core::paths;
use evertranscript_protocol::HistorySearchResponse;
use evertranscript_protocol::Meeting;
use evertranscript_protocol::MeetingDeleteResponse;
use evertranscript_protocol::MeetingDetailResponse;
use evertranscript_protocol::MeetingExportResponse;
use evertranscript_protocol::MeetingListResponse;
use evertranscript_protocol::MeetingResponse;
use evertranscript_protocol::ModelAvailability;
use evertranscript_protocol::ModelsStatusResponse;
use evertranscript_protocol::TranscriptSnapshotResponse;
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
    /// Follow live captions from the Meeting in progress.
    Captions,
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
    runtime.block_on(run(cli))
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Daemon => run_daemon().await,
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
        Command::Captions => run_captions().await,
    }
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

async fn run_daemon() -> Result<()> {
    init_tracing();
    let shutdown = CancellationToken::new();
    let signal_shutdown = shutdown.clone();
    tokio::spawn(async move {
        if let Err(err) = wait_for_shutdown_signal().await {
            tracing::warn!(%err, "signal handling failed; the Core will run until killed");
            return;
        }
        tracing::info!("shutdown signal received");
        signal_shutdown.cancel();
    });

    evertranscript_core::run_daemon(shutdown).await
}

async fn wait_for_shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::signal;
        use tokio::signal::unix::SignalKind;
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
