//! The Core protocol surface: every request a Client may send and every
//! notification the Core may broadcast.
//!
//! The macro-table structure below is PORTED from `openai/codex`
//! (`codex-rs/app-server-protocol/src/protocol/common.rs`, `client_request_definitions!`),
//! Copyright OpenAI, licensed Apache-2.0, pinned rev `5f49aba`. The method
//! tables themselves are EverTranscript's own. See `PORTS.md`.
//!
//! Evolution rule (ADR-0028): changes stay additive. A new method or a new
//! optional field is fine; removing or retyping one is a protocol break and
//! needs a capability gate.

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use crate::rpc::RequestId;

/// Raised when a wire message cannot be turned into a typed request.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("unknown method: {0}")]
    UnknownMethod(String),
    #[error("invalid params for {method}: {source}")]
    InvalidParams {
        method: &'static str,
        #[source]
        source: serde_json::Error,
    },
}

/// Generates the `ClientRequest` enum, its method table, and its wire decoder.
macro_rules! client_request_definitions {
    (
        $(
            $(#[$meta:meta])*
            $variant:ident => $method:literal {
                params: $params:ty,
                response: $response:ty,
            },
        )*
    ) => {
        /// Every request a Client may send the Core.
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
        #[serde(tag = "method", content = "params", rename_all = "camelCase")]
        #[ts(export)]
        pub enum ClientRequest {
            $(
                $(#[$meta])*
                #[serde(rename = $method)]
                $variant($params),
            )*
        }

        impl ClientRequest {
            /// Every method name this build understands, in table order.
            pub const METHODS: &'static [&'static str] = &[$($method),*];

            /// The wire method name for this request.
            pub fn method(&self) -> &'static str {
                match self {
                    $(Self::$variant(_) => $method,)*
                }
            }

            /// Decode a wire `(method, params)` pair into a typed request.
            ///
            /// Absent params decode as an empty object, so a method whose
            /// params are all-defaulted can be called with none.
            pub fn from_wire(
                method: &str,
                params: Option<serde_json::Value>,
            ) -> Result<Self, DecodeError> {
                let params = params.unwrap_or_else(|| serde_json::Value::Object(Default::default()));
                let params = if params.is_null() {
                    serde_json::Value::Object(Default::default())
                } else {
                    params
                };
                match method {
                    $(
                        $method => serde_json::from_value::<$params>(params)
                            .map(Self::$variant)
                            .map_err(|source| DecodeError::InvalidParams { method: $method, source }),
                    )*
                    other => Err(DecodeError::UnknownMethod(other.to_string())),
                }
            }

            /// The Rust type name of this request's response, as declared in
            /// the table. Used by the schema fixtures and by humans reading
            /// the protocol surface.
            pub fn response_type_name(&self) -> &'static str {
                match self {
                    $(Self::$variant(_) => stringify!($response),)*
                }
            }

            /// `(method, response type name)` for every method in the table.
            pub const RESPONSE_TYPES: &'static [(&'static str, &'static str)] =
                &[$(($method, stringify!($response))),*];
        }
    };
}

/// Generates the `ServerNotification` enum and its method table.
macro_rules! server_notification_definitions {
    (
        $(
            $(#[$meta:meta])*
            $variant:ident => $method:literal {
                params: $params:ty,
            },
        )*
    ) => {
        /// Everything the Core may push to an attached Client unprompted.
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
        #[serde(tag = "method", content = "params", rename_all = "camelCase")]
        #[ts(export)]
        pub enum ServerNotification {
            $(
                $(#[$meta])*
                #[serde(rename = $method)]
                $variant($params),
            )*
        }

        impl ServerNotification {
            pub const METHODS: &'static [&'static str] = &[$($method),*];

            pub fn method(&self) -> &'static str {
                match self {
                    $(Self::$variant(_) => $method,)*
                }
            }

            /// Split into the `(method, params)` pair the wire carries.
            pub fn to_wire(&self) -> (&'static str, serde_json::Value) {
                let params = match self {
                    $(Self::$variant(params) => serde_json::to_value(params),)*
                }
                .unwrap_or(serde_json::Value::Null);
                (self.method(), params)
            }
        }
    };
}

client_request_definitions! {
    /// Opens the connection. Must be the first request; every other request
    /// before it is refused with `NOT_INITIALIZED`.
    Initialize => "initialize" {
        params: InitializeParams,
        response: InitializeResponse,
    },
    /// Liveness and identity of the running Core.
    Status => "status" {
        params: StatusParams,
        response: StatusResponse,
    },
}

server_notification_definitions! {
    /// The Core's coarse state changed (idle ↔ recording).
    CoreStateChanged => "core/stateChanged" {
        params: CoreStateChangedParams,
    },
}

/// Notifications a Client may send the Core (no response expected).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "method", content = "params", rename_all = "camelCase")]
#[ts(export)]
pub enum ClientNotification {
    /// Sent after a successful `initialize` response is received.
    #[serde(rename = "initialized")]
    Initialized(InitializedParams),
}

impl ClientNotification {
    pub fn from_wire(method: &str, params: Option<serde_json::Value>) -> Result<Self, DecodeError> {
        let params = params.unwrap_or_else(|| serde_json::Value::Object(Default::default()));
        let params = if params.is_null() {
            serde_json::Value::Object(Default::default())
        } else {
            params
        };
        match method {
            "initialized" => serde_json::from_value::<InitializedParams>(params)
                .map(Self::Initialized)
                .map_err(|source| DecodeError::InvalidParams {
                    method: "initialized",
                    source,
                }),
            other => Err(DecodeError::UnknownMethod(other.to_string())),
        }
    }
}

// ---------------------------------------------------------------- parameters

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct InitializeParams {
    pub client_info: ClientInfo,
    #[serde(default)]
    pub capabilities: ClientCapabilities,
}

/// Who is connecting. Recorded in Core logs; never leaves the machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClientCapabilities {
    /// Opts the connection into methods marked experimental. Off by default,
    /// so a stable Client never sees an unstable surface.
    #[serde(default)]
    pub experimental_api: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct InitializeResponse {
    pub server_info: ServerInfo,
    pub capabilities: ServerCapabilities,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
    /// Wire-protocol version. Bumped only on a breaking change (ADR-0028
    /// keeps changes additive, so this is expected to stay at 1).
    #[ts(type = "number")]
    pub protocol_version: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ServerCapabilities {
    /// True when the Core granted the connection's experimental-API request.
    #[serde(default)]
    pub experimental_api: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct InitializedParams {}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StatusParams {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StatusResponse {
    pub version: String,
    #[ts(type = "number")]
    pub pid: u32,
    #[ts(type = "number")]
    pub uptime_seconds: u64,
    pub state: CoreState,
    /// Absolute path of the History folder this Core writes to.
    pub history_dir: String,
    /// Set when the History folder has Mirrors but no machine store —
    /// an incomplete copy (ADR-0035).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub incomplete_copy_warning: Option<String>,
}

/// The Core's coarse, observable state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CoreState {
    #[default]
    Idle,
    Recording,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CoreStateChangedParams {
    pub state: CoreState,
}

/// Convenience: the id type re-exported for clients building requests.
pub type ClientRequestId = RequestId;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_method_decodes_from_its_own_name() {
        for method in ClientRequest::METHODS {
            let decoded = ClientRequest::from_wire(method, None);
            // `initialize` requires params; everything else must accept none.
            if *method == "initialize" {
                assert!(matches!(decoded, Err(DecodeError::InvalidParams { .. })));
            } else {
                let request =
                    decoded.unwrap_or_else(|e| panic!("{method} rejected empty params: {e}"));
                assert_eq!(&request.method(), method);
            }
        }
    }

    #[test]
    fn unknown_methods_are_named_in_the_error() {
        let error = ClientRequest::from_wire("nope", None).unwrap_err();
        assert!(matches!(error, DecodeError::UnknownMethod(m) if m == "nope"));
    }

    #[test]
    fn initialize_params_decode_with_default_capabilities() {
        let request = ClientRequest::from_wire(
            "initialize",
            Some(serde_json::json!({
                "clientInfo": { "name": "cli", "version": "0.1.0" }
            })),
        )
        .expect("decodes");
        let ClientRequest::Initialize(params) = request else {
            panic!("wrong variant");
        };
        assert_eq!(params.client_info.name, "cli");
        assert!(!params.capabilities.experimental_api);
    }

    #[test]
    fn notifications_split_into_method_and_params() {
        let notification = ServerNotification::CoreStateChanged(CoreStateChangedParams {
            state: CoreState::Recording,
        });
        let (method, params) = notification.to_wire();
        assert_eq!(method, "core/stateChanged");
        assert_eq!(params, serde_json::json!({ "state": "recording" }));
    }
}
