//! JSON-RPC-shaped envelope for the Core protocol.
//!
//! PORTED from `openai/codex` (`codex-rs/app-server-protocol/src/rpc.rs`),
//! Copyright OpenAI, licensed Apache-2.0, at pinned rev `5f49aba`.
//! Adapted for EverTranscript: trace-context field dropped, `RequestId`
//! and the four message shapes otherwise preserved. See `PORTS.md`.
//!
//! Like upstream, this is JSON-RPC 2.0 *shaped*: we neither send nor expect
//! the `"jsonrpc": "2.0"` field (ADR-0028).

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use std::fmt;
use ts_rs::TS;

/// Identifies a request so its response can be matched to it.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize, JsonSchema, TS,
)]
#[serde(untagged)]
#[ts(export)]
pub enum RequestId {
    String(String),
    #[ts(type = "number")]
    Integer(i64),
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(value) => f.write_str(value),
            Self::Integer(value) => write!(f, "{value}"),
        }
    }
}

/// Any valid protocol object that can be decoded off the wire or encoded onto it.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema, TS)]
#[serde(untagged)]
#[ts(export)]
pub enum JsonRpcMessage {
    Request(JsonRpcRequest),
    Response(JsonRpcResponse),
    Error(JsonRpcError),
    Notification(JsonRpcNotification),
}

/// A request that expects a response.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema, TS)]
#[ts(export)]
pub struct JsonRpcRequest {
    pub id: RequestId,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub params: Option<serde_json::Value>,
}

/// A notification, which expects no response.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema, TS)]
#[ts(export)]
pub struct JsonRpcNotification {
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub params: Option<serde_json::Value>,
}

/// A successful response to a request.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema, TS)]
#[ts(export)]
pub struct JsonRpcResponse {
    pub id: RequestId,
    pub result: serde_json::Value,
}

/// A response indicating the request failed.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema, TS)]
#[ts(export)]
pub struct JsonRpcError {
    pub id: RequestId,
    pub error: JsonRpcErrorBody,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema, TS)]
#[ts(export)]
pub struct JsonRpcErrorBody {
    #[ts(type = "number")]
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub data: Option<serde_json::Value>,
}

/// Error codes. The standard JSON-RPC range plus our own above -32000.
pub mod error_codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    /// The connection sent a request before `initialize`.
    pub const NOT_INITIALIZED: i64 = -32001;
    /// The connection sent `initialize` twice.
    pub const ALREADY_INITIALIZED: i64 = -32002;
    /// The Core is refusing work until a prerequisite is met (model missing,
    /// Briefing not acknowledged).
    pub const NOT_READY: i64 = -32003;
}

impl JsonRpcError {
    pub fn new(id: RequestId, code: i64, message: impl Into<String>) -> Self {
        Self {
            id,
            error: JsonRpcErrorBody {
                code,
                message: message.into(),
                data: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_without_a_jsonrpc_field() {
        let line = r#"{"id":1,"method":"status","params":null}"#;
        let message: JsonRpcMessage = serde_json::from_str(line).expect("parses");
        let JsonRpcMessage::Request(request) = &message else {
            panic!("expected a request, got {message:?}");
        };
        assert_eq!(request.method, "status");
        assert_eq!(request.id, RequestId::Integer(1));

        let encoded = serde_json::to_string(&message).expect("serializes");
        assert!(
            !encoded.contains("jsonrpc"),
            "the envelope must not carry a jsonrpc field: {encoded}"
        );
    }

    #[test]
    fn notifications_are_distinguished_from_requests_by_the_absent_id() {
        let message: JsonRpcMessage =
            serde_json::from_str(r#"{"method":"initialized"}"#).expect("parses");
        assert!(matches!(message, JsonRpcMessage::Notification(_)));
    }

    #[test]
    fn string_ids_survive_the_round_trip() {
        let message: JsonRpcMessage =
            serde_json::from_str(r#"{"id":"abc","method":"status"}"#).expect("parses");
        let JsonRpcMessage::Request(request) = message else {
            panic!("expected a request");
        };
        assert_eq!(request.id, RequestId::String("abc".to_string()));
    }
}
