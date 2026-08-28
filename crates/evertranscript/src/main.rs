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
    }
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
