//! A Client of the Core, used by the CLI and by tests.
//!
//! Deliberately thin: connect, initialize, send typed requests, read
//! responses. Notifications arriving between a request and its response are
//! surfaced to the caller rather than dropped, so a caption stream and a
//! command can share one connection.

use anyhow::anyhow;
use anyhow::Context;
use anyhow::Result;
use evertranscript_protocol::ClientCapabilities;
use evertranscript_protocol::ClientInfo;
use evertranscript_protocol::InitializeParams;
use evertranscript_protocol::InitializeResponse;
use evertranscript_protocol::JsonRpcMessage;
use evertranscript_protocol::JsonRpcNotification;
use evertranscript_protocol::JsonRpcRequest;
use evertranscript_protocol::RequestId;
use evertranscript_protocol::StatusResponse;
use serde::de::DeserializeOwned;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::io::ReadHalf;
use tokio::io::WriteHalf;

use crate::paths;

#[cfg(unix)]
type Stream = tokio::net::UnixStream;
#[cfg(windows)]
type Stream = tokio::net::windows::named_pipe::NamedPipeClient;

/// A connection to a running Core.
pub struct CoreClient {
    reader: tokio::io::Lines<BufReader<ReadHalf<Stream>>>,
    writer: WriteHalf<Stream>,
    next_id: i64,
}

impl CoreClient {
    /// Connects to the Core at its default address.
    pub async fn connect() -> Result<Self> {
        #[cfg(unix)]
        let stream = crate::transport::connect(&paths::socket_path())
            .await
            .with_context(|| {
                format!(
                    "no Core is listening at {} — start it with `evertranscript daemon`",
                    paths::socket_path().display()
                )
            })?;
        #[cfg(windows)]
        let stream = crate::transport::connect(&paths::pipe_name())
            .await
            .with_context(|| {
                format!(
                    "no Core is listening at {} — start it with `evertranscript daemon`",
                    paths::pipe_name()
                )
            })?;
        Ok(Self::from_stream(stream))
    }

    /// Connects to a Core at an explicit address. Tests use this so they
    /// never depend on process-global paths.
    #[cfg(unix)]
    pub async fn connect_to(socket_path: &std::path::Path) -> Result<Self> {
        let stream = crate::transport::connect(socket_path)
            .await
            .with_context(|| format!("no Core is listening at {}", socket_path.display()))?;
        Ok(Self::from_stream(stream))
    }

    /// Connects to a Core at an explicit named pipe.
    #[cfg(windows)]
    pub async fn connect_to(pipe_name: &str) -> Result<Self> {
        let stream = crate::transport::connect(pipe_name)
            .await
            .with_context(|| format!("no Core is listening at {pipe_name}"))?;
        Ok(Self::from_stream(stream))
    }

    fn from_stream(stream: Stream) -> Self {
        let (reader, writer) = tokio::io::split(stream);
        Self {
            reader: BufReader::new(reader).lines(),
            writer,
            next_id: 1,
        }
    }

    /// Opens the connection. Every other request is refused before this.
    pub async fn initialize(&mut self, name: &str, version: &str) -> Result<InitializeResponse> {
        self.request(
            "initialize",
            Some(serde_json::to_value(InitializeParams {
                client_info: ClientInfo {
                    name: name.to_string(),
                    version: version.to_string(),
                },
                capabilities: ClientCapabilities::default(),
            })?),
        )
        .await
    }

    /// Connect + initialize in one step, the shape every CLI command wants.
    pub async fn connect_initialized(name: &str) -> Result<Self> {
        let mut client = Self::connect().await?;
        client
            .initialize(name, evertranscript_protocol::VERSION)
            .await?;
        Ok(client)
    }

    pub async fn status(&mut self) -> Result<StatusResponse> {
        self.request("status", None).await
    }

    /// Sends a request and waits for its response, returning any
    /// notifications that arrived first.
    pub async fn request_with_notifications<T: DeserializeOwned>(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<(T, Vec<JsonRpcNotification>)> {
        let id = RequestId::Integer(self.next_id);
        self.next_id += 1;

        let request = JsonRpcMessage::Request(JsonRpcRequest {
            id: id.clone(),
            method: method.to_string(),
            params,
        });
        let mut line = serde_json::to_string(&request)?;
        line.push('\n');
        self.writer.write_all(line.as_bytes()).await?;
        self.writer.flush().await?;

        let mut notifications = Vec::new();
        loop {
            let Some(line) = self.reader.next_line().await? else {
                return Err(anyhow!("the Core closed the connection"));
            };
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<JsonRpcMessage>(&line)
                .with_context(|| format!("unparseable line from the Core: {line}"))?
            {
                JsonRpcMessage::Response(response) if response.id == id => {
                    let value = serde_json::from_value(response.result)
                        .with_context(|| format!("unexpected response shape for {method}"))?;
                    return Ok((value, notifications));
                }
                JsonRpcMessage::Error(error) if error.id == id => {
                    return Err(anyhow!(
                        "{method} failed ({}): {}",
                        error.error.code,
                        error.error.message
                    ));
                }
                JsonRpcMessage::Notification(notification) => notifications.push(notification),
                _ => continue,
            }
        }
    }

    pub async fn request<T: DeserializeOwned>(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<T> {
        let (value, _notifications) = self.request_with_notifications(method, params).await?;
        Ok(value)
    }

    /// Reads the next notification pushed by the Core.
    pub async fn next_notification(&mut self) -> Result<Option<JsonRpcNotification>> {
        loop {
            let Some(line) = self.reader.next_line().await? else {
                return Ok(None);
            };
            if line.trim().is_empty() {
                continue;
            }
            if let JsonRpcMessage::Notification(notification) =
                serde_json::from_str::<JsonRpcMessage>(&line)?
            {
                return Ok(Some(notification));
            }
        }
    }
}
