//! The Core's local listener: newline-delimited JSON over a unix socket
//! (macOS) or a named pipe (Windows), serving 0..N concurrent Clients.
//!
//! The socket lifecycle — refuse-if-live, clean-if-stale, 0600 permissions,
//! unlink-on-drop, and the startup lock that serializes competing launches —
//! is PORTED from `openai/codex`
//! (`codex-rs/app-server-transport/src/transport/unix_socket.rs`), Copyright
//! OpenAI, licensed Apache-2.0, pinned rev `5f49aba`. The JSONL framing
//! follows the same source's `stdio.rs`. See `PORTS.md`.
//!
//! Deviation from upstream (ADR-0028): upstream speaks WebSocket frames over
//! the unix socket; we speak plain JSONL, because our GUI Client is Node and
//! an on-machine handshake buys nothing. Windows gets a named pipe rather
//! than AF_UNIX for the same reason — libuv has no AF_UNIX there.

use std::io::ErrorKind;
use std::io::Result as IoResult;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use evertranscript_protocol::JsonRpcMessage;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::debug;
use tracing::error;
use tracing::info;
use tracing::warn;

/// Outbound queue depth per connection. A Client that falls this far behind
/// on ordinary traffic is not keeping up; caption deltas get their own lossy
/// policy rather than sharing this backpressure (ADR-0028).
pub const CHANNEL_CAPACITY: usize = 256;

/// Identifies one attached Client for the lifetime of its connection.
pub type ConnectionId = u64;

static NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);

fn next_connection_id() -> ConnectionId {
    NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed)
}

/// What the transport tells the server about the world.
#[derive(Debug)]
pub enum TransportEvent {
    Opened {
        connection_id: ConnectionId,
        writer: mpsc::Sender<JsonRpcMessage>,
    },
    Line {
        connection_id: ConnectionId,
        line: String,
    },
    Closed {
        connection_id: ConnectionId,
    },
}

/// Serves one connected stream: a reader task turning lines into events and
/// a writer task turning messages into lines. Returns when either ends.
async fn serve_stream<S>(
    stream: S,
    events: mpsc::Sender<TransportEvent>,
    shutdown: CancellationToken,
) where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let connection_id = next_connection_id();
    let (reader, mut writer) = tokio::io::split(stream);
    let (writer_tx, mut writer_rx) = mpsc::channel::<JsonRpcMessage>(CHANNEL_CAPACITY);

    if events
        .send(TransportEvent::Opened {
            connection_id,
            writer: writer_tx,
        })
        .await
        .is_err()
    {
        return;
    }
    debug!(connection_id, "client connected");

    let write_task = tokio::spawn(async move {
        while let Some(message) = writer_rx.recv().await {
            let Ok(mut line) = serde_json::to_string(&message) else {
                error!("failed to serialize an outgoing message; dropping it");
                continue;
            };
            line.push('\n');
            if let Err(err) = writer.write_all(line.as_bytes()).await {
                debug!(%err, "client write failed; closing connection");
                break;
            }
            if let Err(err) = writer.flush().await {
                debug!(%err, "client flush failed; closing connection");
                break;
            }
        }
    });

    let mut lines = BufReader::new(reader).lines();
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            next = lines.next_line() => match next {
                Ok(Some(line)) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    if events
                        .send(TransportEvent::Line { connection_id, line })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(None) => break,
                Err(err) => {
                    debug!(%err, "client read failed");
                    break;
                }
            },
        }
    }

    write_task.abort();
    let _ = events.send(TransportEvent::Closed { connection_id }).await;
    debug!(connection_id, "client disconnected");
}

// ------------------------------------------------------------------- unix

#[cfg(unix)]
mod platform {
    use super::*;
    use std::path::Path;
    use std::path::PathBuf;
    use tokio::net::UnixListener;
    use tokio::net::UnixStream;

    const SOCKET_MODE: u32 = 0o600;

    /// Holds the listener and unlinks the socket file on drop.
    #[derive(Debug)]
    pub struct Listener {
        listener: UnixListener,
        socket_path: PathBuf,
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            if let Err(err) = std::fs::remove_file(&self.socket_path)
                && err.kind() != ErrorKind::NotFound
            {
                warn!(path = %self.socket_path.display(), %err, "failed to remove socket file");
            }
        }
    }

    /// Binds the listener, refusing to start if another Core already holds
    /// the socket and cleaning it up if it is merely stale.
    pub async fn bind(socket_path: &Path) -> IoResult<Listener> {
        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        prepare_socket_path(socket_path).await?;
        let listener = UnixListener::bind(socket_path)?;
        set_socket_permissions(socket_path)?;
        info!(path = %socket_path.display(), "Core listening");
        Ok(Listener {
            listener,
            socket_path: socket_path.to_path_buf(),
        })
    }

    /// Ported refuse-or-clean discipline: a socket that still accepts
    /// connections belongs to a live Core; one that refuses is stale.
    async fn prepare_socket_path(socket_path: &Path) -> IoResult<()> {
        match UnixStream::connect(socket_path).await {
            Ok(_) => Err(std::io::Error::new(
                ErrorKind::AddrInUse,
                format!(
                    "another EverTranscript Core is already listening at {}",
                    socket_path.display()
                ),
            )),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
            Err(err) if err.kind() == ErrorKind::ConnectionRefused => {
                // Stale socket from a Core that died without unlinking.
                match std::fs::remove_file(socket_path) {
                    Ok(()) => Ok(()),
                    Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
                    Err(err) => Err(err),
                }
            }
            Err(err) => {
                if socket_path.exists() {
                    Err(err)
                } else {
                    Ok(())
                }
            }
        }
    }

    fn set_socket_permissions(socket_path: &Path) -> IoResult<()> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(SOCKET_MODE))
    }

    /// Accepts connections until shutdown.
    pub async fn serve(
        listener: Listener,
        events: mpsc::Sender<TransportEvent>,
        shutdown: CancellationToken,
    ) {
        loop {
            let stream = tokio::select! {
                _ = shutdown.cancelled() => break,
                accepted = listener.listener.accept() => match accepted {
                    Ok((stream, _addr)) => stream,
                    Err(err) if matches!(
                        err.kind(),
                        ErrorKind::ConnectionAborted | ErrorKind::ConnectionReset | ErrorKind::Interrupted
                    ) => {
                        warn!(%err, "recoverable accept error");
                        continue;
                    }
                    Err(err) => {
                        error!(%err, "accept failed");
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        continue;
                    }
                },
            };
            let events = events.clone();
            let shutdown = shutdown.clone();
            tokio::spawn(serve_stream(stream, events, shutdown));
        }
        info!("listener shutting down");
    }

    /// Connects to a running Core. Used by the CLI and by tests.
    pub async fn connect(socket_path: &Path) -> IoResult<UnixStream> {
        UnixStream::connect(socket_path).await
    }
}

// ---------------------------------------------------------------- windows

#[cfg(windows)]
mod platform {
    use super::*;
    use tokio::net::windows::named_pipe::ClientOptions;
    use tokio::net::windows::named_pipe::NamedPipeClient;
    use tokio::net::windows::named_pipe::NamedPipeServer;
    use tokio::net::windows::named_pipe::ServerOptions;

    #[derive(Debug)]
    pub struct Listener {
        pipe_name: String,
        first: Option<NamedPipeServer>,
    }

    /// Creating the first instance with `first_pipe_instance` is the Windows
    /// equivalent of the unix refuse-if-live check: a second Core fails here.
    pub async fn bind(pipe_name: &str) -> IoResult<Listener> {
        let first = ServerOptions::new()
            .first_pipe_instance(true)
            .create(pipe_name)
            .map_err(|err| {
                if err.kind() == ErrorKind::PermissionDenied {
                    std::io::Error::new(
                        ErrorKind::AddrInUse,
                        format!("another EverTranscript Core is already listening at {pipe_name}"),
                    )
                } else {
                    err
                }
            })?;
        info!(pipe = pipe_name, "Core listening");
        Ok(Listener {
            pipe_name: pipe_name.to_string(),
            first: Some(first),
        })
    }

    pub async fn serve(
        mut listener: Listener,
        events: mpsc::Sender<TransportEvent>,
        shutdown: CancellationToken,
    ) {
        let mut server = match listener.first.take() {
            Some(server) => server,
            None => return,
        };
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                connected = server.connect() => {
                    if let Err(err) = connected {
                        error!(%err, "named pipe connect failed");
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        continue;
                    }
                }
            }
            // Hand off the connected instance and immediately open the next,
            // so there is never a window where a Client finds no pipe.
            let connected = std::mem::replace(
                &mut server,
                match ServerOptions::new().create(&listener.pipe_name) {
                    Ok(next) => next,
                    Err(err) => {
                        error!(%err, "failed to create the next pipe instance");
                        break;
                    }
                },
            );
            let events = events.clone();
            let shutdown_for_conn = shutdown.clone();
            tokio::spawn(serve_stream(connected, events, shutdown_for_conn));
        }
        info!("listener shutting down");
    }

    pub async fn connect(pipe_name: &str) -> IoResult<NamedPipeClient> {
        ClientOptions::new().open(pipe_name)
    }
}

pub use platform::Listener;
pub use platform::bind;
pub use platform::connect;
pub use platform::serve;

/// Holds the startup lock for the lifetime of the process.
///
/// Ported discipline: two Cores racing to bind must not both believe they
/// cleaned a stale socket. The lock serializes the check-and-bind.
pub struct StartupLock {
    _file: std::fs::File,
}

/// Acquires the startup lock, blocking briefly if another launch holds it.
pub async fn acquire_startup_lock(path: std::path::PathBuf) -> IoResult<StartupLock> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    tokio::task::spawn_blocking(move || {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)?;
        file.lock()?;
        Ok(StartupLock { _file: file })
    })
    .await
    .map_err(|err| std::io::Error::other(format!("startup lock task failed: {err}")))?
}
