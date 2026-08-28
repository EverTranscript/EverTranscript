//! The EverTranscript Core.
//!
//! One always-on process that owns detection, capture, transcription, and
//! storage, and is the record's only writer (ADR-0026). Every surface — the
//! Electron Client, the CLI — is a Client of this crate over the protocol in
//! `evertranscript-protocol`, never a second writer.

pub mod asr;
pub mod audio;
pub mod autostart;
pub mod client;
pub mod detect;
pub mod mirror;
pub mod models;
pub mod paths;
pub mod server;
pub mod settings;
pub mod store;
pub mod transport;
pub mod tray;

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

    // Before serving anything: merge audio checkpoints a previous run left
    // behind. A Core that was killed mid-meeting gets its recording back,
    // minus at most the checkpoint that was in flight.
    core.recover_interrupted_audio().await;

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

    let serving = tokio::spawn(async move {
        transport::serve(listener, events_tx, shutdown).await;
        let _ = server_task.await;
        let _ = mirror_task.await;
    });

    Ok(Daemon {
        core: Arc::clone(&core),
        serving,
    })
}

/// Brings the Core up and blocks until it stops.
pub async fn run_daemon(shutdown: CancellationToken) -> anyhow::Result<()> {
    start_daemon(shutdown).await?.join().await;
    Ok(())
}
