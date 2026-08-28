//! The message processor: one running Core, 0..N attached Clients.
//!
//! Connection handling follows the codex app-server shape (per-connection
//! `initialize` handshake, typed dispatch, broadcast fanout to initialized
//! connections) with our own method table. Work continues when zero Clients
//! are attached — that property is why the Core is a daemon rather than a
//! child process (ADR-0026).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use evertranscript_protocol::error_codes;
use evertranscript_protocol::ClientNotification;
use evertranscript_protocol::ClientRequest;
use evertranscript_protocol::CoreState;
use evertranscript_protocol::CoreStateChangedParams;
use evertranscript_protocol::InitializeParams;
use evertranscript_protocol::InitializeResponse;
use evertranscript_protocol::JsonRpcError;
use evertranscript_protocol::JsonRpcMessage;
use evertranscript_protocol::JsonRpcResponse;
use evertranscript_protocol::RequestId;
use evertranscript_protocol::ServerCapabilities;
use evertranscript_protocol::ServerInfo;
use evertranscript_protocol::ServerNotification;
use evertranscript_protocol::StatusResponse;
use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::debug;
use tracing::warn;

use crate::paths;
use crate::transport::ConnectionId;
use crate::transport::TransportEvent;

/// The Core's own state, independent of any Client.
pub struct Core {
    started_at: Instant,
    history_dir: std::path::PathBuf,
    state: Mutex<CoreState>,
    /// Set once at startup if the History folder looked like an incomplete
    /// copy. Sticky on purpose: creating the missing store would otherwise
    /// erase the only evidence that the Operator's copy was partial, and
    /// they would never learn their audio and Voiceprints were left behind.
    incomplete_copy: Option<String>,
}

impl Core {
    /// Creates the Core and its History layout.
    ///
    /// A folder holding Mirrors but no machine store is reported rather than
    /// silently re-created: the Operator believes they copied their History,
    /// and a fresh empty store would look like data loss (ADR-0035).
    pub fn new() -> anyhow::Result<Arc<Self>> {
        Self::with_history_dir(paths::history_dir())
    }

    /// Same, against an explicit History folder. Tests use this so they never
    /// depend on process-global paths.
    pub fn with_history_dir(history_dir: std::path::PathBuf) -> anyhow::Result<Arc<Self>> {
        let incomplete_copy = paths::detect_incomplete_copy(&history_dir).then(|| {
            format!(
                "{} holds Mirrors but no machine store — this looks like an incomplete copy, \
                 so transcripts, audio, and Voiceprints may have been left behind. Copy the \
                 whole folder, hidden files included.",
                history_dir.display()
            )
        });
        paths::ensure_history_layout(&history_dir)?;
        if let Some(warning) = &incomplete_copy {
            warn!("{warning}");
        }
        Ok(Arc::new(Self {
            started_at: Instant::now(),
            history_dir,
            state: Mutex::new(CoreState::Idle),
            incomplete_copy,
        }))
    }

    pub fn history_dir(&self) -> &std::path::Path {
        &self.history_dir
    }

    pub fn uptime_seconds(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }

    pub async fn state(&self) -> CoreState {
        *self.state.lock().await
    }

    pub async fn set_state(&self, state: CoreState) {
        *self.state.lock().await = state;
    }

    async fn status(&self) -> StatusResponse {
        StatusResponse {
            version: evertranscript_protocol::VERSION.to_string(),
            pid: std::process::id(),
            uptime_seconds: self.uptime_seconds(),
            state: self.state().await,
            history_dir: self.history_dir.display().to_string(),
            incomplete_copy_warning: self.incomplete_copy.clone(),
        }
    }
}

/// Per-connection state. Evaporates when the Client disconnects; the record
/// and any in-flight work do not.
struct Connection {
    writer: mpsc::Sender<JsonRpcMessage>,
    initialized: bool,
    experimental_api: bool,
}

/// Runs the protocol for every attached Client.
pub struct Server {
    core: Arc<Core>,
    connections: HashMap<ConnectionId, Connection>,
}

impl Server {
    pub fn new(core: Arc<Core>) -> Self {
        Self {
            core,
            connections: HashMap::new(),
        }
    }

    pub fn core(&self) -> &Arc<Core> {
        &self.core
    }

    /// Consumes transport events until the channel closes or shutdown fires.
    pub async fn run(
        mut self,
        mut events: mpsc::Receiver<TransportEvent>,
        shutdown: CancellationToken,
    ) {
        loop {
            let event = tokio::select! {
                _ = shutdown.cancelled() => break,
                event = events.recv() => match event {
                    Some(event) => event,
                    None => break,
                },
            };
            self.handle_event(event).await;
        }
        debug!("server loop finished");
    }

    async fn handle_event(&mut self, event: TransportEvent) {
        match event {
            TransportEvent::Opened {
                connection_id,
                writer,
            } => {
                self.connections.insert(
                    connection_id,
                    Connection {
                        writer,
                        initialized: false,
                        experimental_api: false,
                    },
                );
            }
            TransportEvent::Closed { connection_id } => {
                self.connections.remove(&connection_id);
            }
            TransportEvent::Line {
                connection_id,
                line,
            } => {
                self.handle_line(connection_id, &line).await;
            }
        }
    }

    async fn handle_line(&mut self, connection_id: ConnectionId, line: &str) {
        let message: JsonRpcMessage = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(err) => {
                warn!(connection_id, %err, "unparseable line from client");
                // Without an id there is nobody to answer; the line is dropped.
                return;
            }
        };

        match message {
            JsonRpcMessage::Request(request) => {
                let response = self
                    .dispatch_request(
                        connection_id,
                        request.id.clone(),
                        &request.method,
                        request.params,
                    )
                    .await;
                self.send(connection_id, response).await;
            }
            JsonRpcMessage::Notification(notification) => {
                match ClientNotification::from_wire(&notification.method, notification.params) {
                    Ok(ClientNotification::Initialized(_)) => {
                        debug!(connection_id, "client finished initializing");
                    }
                    Err(err) => {
                        debug!(connection_id, %err, "ignoring unknown client notification");
                    }
                }
            }
            JsonRpcMessage::Response(_) | JsonRpcMessage::Error(_) => {
                // Server-to-client requests do not exist yet, so a response
                // arriving here has nothing to correlate with.
                debug!(connection_id, "ignoring unexpected response from client");
            }
        }
    }

    async fn dispatch_request(
        &mut self,
        connection_id: ConnectionId,
        id: RequestId,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> JsonRpcMessage {
        let request = match ClientRequest::from_wire(method, params) {
            Ok(request) => request,
            Err(evertranscript_protocol::DecodeError::UnknownMethod(method)) => {
                return JsonRpcMessage::Error(JsonRpcError::new(
                    id,
                    error_codes::METHOD_NOT_FOUND,
                    format!("unknown method: {method}"),
                ));
            }
            Err(err) => {
                return JsonRpcMessage::Error(JsonRpcError::new(
                    id,
                    error_codes::INVALID_PARAMS,
                    err.to_string(),
                ));
            }
        };

        let initialized = self
            .connections
            .get(&connection_id)
            .is_some_and(|connection| connection.initialized);

        match (&request, initialized) {
            (ClientRequest::Initialize(_), true) => {
                return JsonRpcMessage::Error(JsonRpcError::new(
                    id,
                    error_codes::ALREADY_INITIALIZED,
                    "this connection is already initialized",
                ));
            }
            (request, false) if !matches!(request, ClientRequest::Initialize(_)) => {
                return JsonRpcMessage::Error(JsonRpcError::new(
                    id,
                    error_codes::NOT_INITIALIZED,
                    "send initialize before any other request",
                ));
            }
            _ => {}
        }

        let result = match request {
            ClientRequest::Initialize(params) => self.handle_initialize(connection_id, params),
            ClientRequest::Status(_) => {
                serde_json::to_value(self.core.status().await).unwrap_or(serde_json::Value::Null)
            }
        };

        JsonRpcMessage::Response(JsonRpcResponse { id, result })
    }

    fn handle_initialize(
        &mut self,
        connection_id: ConnectionId,
        params: InitializeParams,
    ) -> serde_json::Value {
        // Experimental methods are opt-in per connection, so a stable Client
        // never sees an unstable surface (ADR-0028).
        let experimental_api = params.capabilities.experimental_api;
        if let Some(connection) = self.connections.get_mut(&connection_id) {
            connection.initialized = true;
            connection.experimental_api = experimental_api;
        }
        debug!(
            connection_id,
            client = %params.client_info.name,
            version = %params.client_info.version,
            "client initialized"
        );
        serde_json::to_value(InitializeResponse {
            server_info: ServerInfo {
                name: evertranscript_protocol::SERVER_NAME.to_string(),
                version: evertranscript_protocol::VERSION.to_string(),
                protocol_version: evertranscript_protocol::PROTOCOL_VERSION,
            },
            capabilities: ServerCapabilities { experimental_api },
        })
        .unwrap_or(serde_json::Value::Null)
    }

    async fn send(&mut self, connection_id: ConnectionId, message: JsonRpcMessage) {
        let Some(connection) = self.connections.get(&connection_id) else {
            return;
        };
        if connection.writer.send(message).await.is_err() {
            self.connections.remove(&connection_id);
        }
    }

    /// Pushes a notification to every initialized connection. Connections
    /// whose queue is full or closed are dropped rather than blocking the
    /// Core — capture must never wait on a Client.
    pub async fn broadcast(&mut self, notification: ServerNotification) {
        let (method, params) = notification.to_wire();
        let message = JsonRpcMessage::Notification(evertranscript_protocol::JsonRpcNotification {
            method: method.to_string(),
            params: Some(params),
        });
        let mut dead = Vec::new();
        for (connection_id, connection) in &self.connections {
            if !connection.initialized {
                continue;
            }
            if connection.writer.try_send(message.clone()).is_err() {
                dead.push(*connection_id);
            }
        }
        for connection_id in dead {
            debug!(connection_id, "dropping a client that fell behind");
            self.connections.remove(&connection_id);
        }
    }

    /// Sets Core state and tells everyone attached.
    pub async fn set_state(&mut self, state: CoreState) {
        self.core.set_state(state).await;
        self.broadcast(ServerNotification::CoreStateChanged(
            CoreStateChangedParams { state },
        ))
        .await;
    }
}
