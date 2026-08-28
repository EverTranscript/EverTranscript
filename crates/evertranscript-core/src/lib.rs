//! The EverTranscript Core.
//!
//! One always-on process that owns detection, capture, transcription, and
//! storage, and is the record's only writer (ADR-0026). Every surface — the
//! Electron Client, the CLI — is a Client of this crate over the protocol in
//! `evertranscript-protocol`, never a second writer.

pub mod client;
pub mod paths;
pub mod server;
pub mod transport;

pub use server::Core;
pub use server::Server;

use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Boots the Core: acquires the startup lock, binds the listener, and serves
/// until shutdown. Returns when the listener stops.
pub async fn run_daemon(shutdown: CancellationToken) -> anyhow::Result<()> {
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
    let (events_tx, events_rx) = mpsc::channel(transport::CHANNEL_CAPACITY);

    let server = Server::new(Arc::clone(&core));
    let server_shutdown = shutdown.clone();
    let server_task = tokio::spawn(server.run(events_rx, server_shutdown));

    transport::serve(listener, events_tx, shutdown).await;
    let _ = server_task.await;
    Ok(())
}
