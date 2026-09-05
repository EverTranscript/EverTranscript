//! The EverTranscript Core.
//!
//! One always-on process that owns detection, capture, transcription, and
//! storage, and is the record's only writer (ADR-0026). Every surface — the
//! Electron Client, the CLI — is a Client of this crate over the protocol in
//! `evertranscript-protocol`, never a second writer.

pub mod asr;
pub mod audio;
pub mod autostart;
pub mod briefing;
pub mod client;
pub mod detect;
pub mod diarize;
pub mod mirror;
pub mod models;
pub mod paths;
pub mod posture;
pub mod server;
pub mod settings;
pub mod store;
pub mod summary;
pub mod transport;
pub mod tray;
pub mod updates;

pub use server::Core;
pub use server::Server;

use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Boots the Core: acquires the startup lock, binds the listener, and serves
/// until shutdown. Returns when the listener stops.
/// A Core that is up and serving.
///
/// Handed back so the caller can decide what the main thread does next. That
/// only matters on macOS, where the menu bar demands the main thread and the
/// async work has to move off it (ADR-0023) — but the split is worth having
/// either way, because "the Core is running" and "this thread is blocked
/// until it stops" are separate facts.
pub struct Daemon {
    core: Arc<Core>,
    serving: tokio::task::JoinHandle<()>,
}

impl Daemon {
    pub fn core(&self) -> &Arc<Core> {
        &self.core
    }

    /// Waits for the Core to finish shutting down.
    pub async fn join(self) {
        let _ = self.serving.await;
    }
}

/// Brings the Core up without blocking the caller.
pub async fn start_daemon(shutdown: CancellationToken) -> anyhow::Result<Daemon> {
    // The startup lock covers exactly the check-and-bind window, so two
    // launches racing to clean a stale socket cannot both believe they won.
    // It is released immediately afterwards: holding it for the process
    // lifetime would make a second launch block forever instead of learning
    // that a Core is already listening.
    let listener = {
        let _startup_lock = transport::acquire_startup_lock(paths::startup_lock_path()).await?;
        #[cfg(unix)]
        let listener = transport::bind(&paths::socket_path()).await?;
        #[cfg(windows)]
        let listener = transport::bind(&paths::pipe_name()).await?;
        listener
    };

    let core = Core::new()?;

    // Before serving anything: settle whatever a previous run left half
    // done. A Core that was killed mid-meeting gets its recording back, minus
    // at most the checkpoint that was in flight — and the Meeting it was
    // killed inside gets closed and told what it lost, rather than staying
    // open and blocking the next one.
    core.reconcile_after_restart().await;

    let (events_tx, events_rx) = mpsc::channel(transport::CHANNEL_CAPACITY);

    // The Mirror projection runs alongside the server, not inside it: a slow
    // disk must never make a Client's request wait.
    let mirror_task = tokio::spawn(
        core.mirror()
            .clone()
            .run(core.mirror_wake(), shutdown.clone()),
    );

    let server = Server::new(Arc::clone(&core));
    let server_shutdown = shutdown.clone();
    let server_task = tokio::spawn(server.run(events_rx, server_shutdown));

    // Meeting Detection runs beside the server, never inside it: a policy
    // deciding what to record must not be able to make a Client's request
    // wait, and a Core with no detector must still serve (ADR-0026).
    let senses = detect_sources();
    let detection_task = (!senses.is_empty()).then(|| {
        tokio::spawn(detect::driver::run(
            Arc::clone(&core),
            senses,
            Box::new(detect::notify::SilentNotifier),
            shutdown.clone(),
        ))
    });

    let serving = tokio::spawn(async move {
        transport::serve(listener, events_tx, shutdown).await;
        let _ = server_task.await;
        let _ = mirror_task.await;
        if let Some(detection) = detection_task {
            let _ = detection.await;
        }
    });

    Ok(Daemon {
        core: Arc::clone(&core),
        serving,
    })
}

/// This platform's live detector, when it has one.
///
/// `None` is a supported answer: a Core on a platform without detection
/// serves normally and records when asked, exactly as M1 did. Auto-Record is
/// the thing that is missing, not the product.
fn detect_sources() -> Vec<Box<dyn detect::DetectionSource>> {
    #[cfg(target_os = "macos")]
    let machine: Box<dyn detect::DetectionSource> =
        Box::new(detect::macos::MacOsDetectionSource::new());
    #[cfg(target_os = "windows")]
    let machine: Box<dyn detect::DetectionSource> =
        Box::new(detect::windows::WindowsDetectionSource::new());

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        // Arms and names; never triggers (ADR-0036). Starting the calendar
        // without a grant is a no-op rather than an error.
        vec![machine, Box::new(detect::calendar::CalendarSource::new())]
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Vec::new()
    }
}

/// Brings the Core up and blocks until it stops.
pub async fn run_daemon(shutdown: CancellationToken) -> anyhow::Result<()> {
    start_daemon(shutdown).await?.join().await;
    Ok(())
}
