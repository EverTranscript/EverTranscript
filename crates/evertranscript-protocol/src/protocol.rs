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
        //
        // Variants differ in size (a Meeting is far bigger than a state
        // enum), but a notification exists only long enough to be turned
        // into a wire value and is never stored, so boxing every variant
        // would trade an allocation per notification for stack bytes that
        // are already gone by the next line.
        #[allow(clippy::large_enum_variant)]
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
    /// Starts recording. In M1 this is the Operator's explicit act; from M2
    /// Auto-Record calls the same path.
    MeetingStart => "meeting/start" {
        params: MeetingStartParams,
        response: MeetingResponse,
    },
    /// Stops the Meeting in progress and persists it.
    MeetingStop => "meeting/stop" {
        params: MeetingStopParams,
        response: MeetingResponse,
    },
    /// Most recent Meetings first.
    MeetingList => "meeting/list" {
        params: MeetingListParams,
        response: MeetingListResponse,
    },
    /// One Meeting with its Transcript.
    MeetingGet => "meeting/get" {
        params: MeetingGetParams,
        response: MeetingDetailResponse,
    },
    /// Renames a Meeting. The Mirror follows, and the old filename is
    /// garbage-collected by its id8 (ADR-0035).
    MeetingRetitle => "meeting/retitle" {
        params: MeetingRetitleParams,
        response: MeetingResponse,
    },
    /// Removes a Meeting entirely — rows, Mirror, and audio. The only
    /// destructive operation on the record besides Voiceprint deletion.
    MeetingDelete => "meeting/delete" {
        params: MeetingDeleteParams,
        response: MeetingDeleteResponse,
    },
    /// The Meeting's Mirror markdown, exactly as written to disk.
    MeetingExport => "meeting/export" {
        params: MeetingGetParams,
        response: MeetingExportResponse,
    },
    /// Full-text search across History.
    HistorySearch => "history/search" {
        params: HistorySearchParams,
        response: HistorySearchResponse,
    },
    /// What the Core has on disk and what it still needs.
    ModelsStatus => "models/status" {
        params: ModelsStatusParams,
        response: ModelsStatusResponse,
    },
    /// Downloads missing models. Returns once they are verified in place.
    ModelsFetch => "models/fetch" {
        params: ModelsFetchParams,
        response: ModelsStatusResponse,
    },
    /// Subscribes to live captions and returns the transcript so far, in one
    /// call.
    ///
    /// Snapshot and subscription together on purpose: fetching the
    /// transcript and then subscribing leaves a window where a segment
    /// completes between the two and is lost from the Client's view forever
    /// (ADR-0028, snapshot-then-tail).
    TranscriptSubscribe => "transcript/subscribe" {
        params: TranscriptSubscribeParams,
        response: TranscriptSnapshotResponse,
    },
    /// Stops caption delivery for this connection.
    TranscriptUnsubscribe => "transcript/unsubscribe" {
        params: TranscriptUnsubscribeParams,
        response: TranscriptUnsubscribeResponse,
    },
    /// This installation's settings.
    SettingsGet => "settings/get" {
        params: SettingsGetParams,
        response: SettingsResponse,
    },
    /// Changes settings. Every field is optional; omitted fields are left
    /// alone, so a Client can toggle one thing without echoing the rest.
    SettingsSet => "settings/set" {
        params: SettingsSetParams,
        response: SettingsResponse,
    },
    /// What Meeting Detection watches on this machine, and what it offers.
    WatchlistGet => "watchlist/get" {
        params: WatchlistGetParams,
        response: WatchlistResponse,
    },
    /// Adds an app to the Watchlist. Membership is the per-app switch
    /// (ADR-0030): there is no enable flag to set afterwards.
    WatchlistAdd => "watchlist/add" {
        params: WatchlistAddParams,
        response: WatchlistResponse,
    },
    /// Removes an app from the Watchlist.
    WatchlistRemove => "watchlist/remove" {
        params: WatchlistRemoveParams,
        response: WatchlistResponse,
    },
    /// Every Speaker the app holds — the Voice Registry's inventory
    /// (story 30). ADR-0008 makes this surface mandatory rather than
    /// optional: it is half of what was traded for storing Voiceprints
    /// at all.
    SpeakerList => "speaker/list" {
        params: SpeakerListParams,
        response: SpeakerListResponse,
    },
    /// One Speaker, with the facts a person needs to act on it.
    SpeakerGet => "speaker/get" {
        params: SpeakerGetParams,
        response: SpeakerDetailResponse,
    },
    /// Names a Speaker, which also confirms its Voiceprint (ADR-0008 as
    /// amended). Retroactive across all of History by construction — the
    /// record holds references, not names (story 29).
    SpeakerRename => "speaker/rename" {
        params: SpeakerRenameParams,
        response: SpeakerResponse,
    },
    /// Deletes a Speaker's Voiceprint (story 31). Recognition stops; the
    /// record is untouched. The only destructive biometric operation there
    /// is (ADR-0009) — the Speaker itself is permanent.
    SpeakerDeleteVoiceprint => "speaker/deleteVoiceprint" {
        params: SpeakerGetParams,
        response: SpeakerResponse,
    },
    /// Re-assigns a segment to a different Speaker (story 29b). Appends a
    /// hint; the machine's attribution is preserved beneath it.
    TranscriptReassign => "transcript/reassign" {
        params: TranscriptReassignParams,
        response: TranscriptReassignResponse,
    },
    /// What Diarization is doing, if anything.
    DiarizeStatus => "diarize/status" {
        params: DiarizeStatusParams,
        response: DiarizeStatusResponse,
    },
    /// Diarizes a finished Meeting, or re-diarizes one.
    ///
    /// Re-running replaces the machine's attribution and preserves every
    /// Operator correction (ADR-0009 as amended) — which is what makes a
    /// model upgrade safe to apply to a History that has been corrected.
    DiarizeRun => "diarize/run" {
        params: DiarizeCancelParams,
        response: DiarizeStatusResponse,
    },
    /// Stops a running Diarization. Whatever attribution completed is kept;
    /// a job the Operator cannot stop is an unaccountable use of their
    /// machine.
    DiarizeCancel => "diarize/cancel" {
        params: DiarizeCancelParams,
        response: DiarizeStatusResponse,
    },
}

server_notification_definitions! {
    /// The Core's coarse state changed (idle ↔ recording).
    CoreStateChanged => "core/stateChanged" {
        params: CoreStateChangedParams,
    },
    /// A Meeting was created, updated, or removed. Clients refresh from it
    /// rather than polling.
    MeetingChanged => "meeting/changed" {
        params: MeetingChangedParams,
    },
    /// A new Transcript segment. Delivered only to subscribed connections,
    /// and delivered lossily: a Client that falls behind loses captions
    /// rather than slowing capture or being disconnected (ADR-0028).
    TranscriptSegmentAdded => "transcript/segmentAdded" {
        params: TranscriptSegmentAddedParams,
    },
    /// Some captions were dropped because this connection was too slow.
    /// Sent so a Client can show a gap rather than silently missing words.
    TranscriptCaptionsDropped => "transcript/captionsDropped" {
        params: TranscriptCaptionsDroppedParams,
    },
    /// A Speaker was created, renamed, or had its Voiceprint deleted. A
    /// rename is retroactive across every Meeting, so a Client showing any
    /// transcript needs to know rather than poll.
    SpeakerChanged => "speaker/changed" {
        params: SpeakerChangedParams,
    },
    /// Diarization progressed, finished, or stopped. Post-meeting work the
    /// Operator did not ask for out loud still has to be visible while it
    /// runs.
    DiarizeProgress => "diarize/progress" {
        params: DiarizeProgressParams,
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

// ------------------------------------------------------------------ Meetings

/// One recorded session: the unit of storage, retrieval, and summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Meeting {
    /// UUIDv7. Its first 8 hex characters appear in the Mirror's filename
    /// and are the stable handle outside references should key on.
    pub id: String,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ended_at: Option<String>,
    /// None until titled. Clients render the fallback rather than inventing
    /// a title, so "untitled" never becomes a stored string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub title: Option<String>,
    /// The calendar event this Meeting was armed from, when one named it
    /// (ADR-0036). Kept so the record can say *which* scheduled meeting
    /// this was, not merely that it had a title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub calendar_event_id: Option<String>,
    /// Who the event invited. **Stored, not applied**: these become
    /// Speaker-naming suggestions in M3, and turning an invitation into an
    /// attribution before Diarization exists would be inventing who spoke.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calendar_attendees: Vec<String>,
    /// The app detection attributed this Meeting to; the Mirror's slug
    /// before a Title exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub detected_app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub duration_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub mirror_filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub audio_path: Option<String>,
    /// What this recording lost, in the Operator's terms — a capture leg that
    /// never started, one that died partway, audio that would not encode.
    ///
    /// Empty for a whole recording. This is in the record rather than only in
    /// a log because a Meeting missing half its audio is otherwise
    /// indistinguishable from one where nobody spoke, and the Operator finds
    /// out by reading a transcript that makes no sense.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audio_notes: Vec<String>,
}

/// One attributed span of the Transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TranscriptSegment {
    pub id: String,
    #[ts(type = "number")]
    pub sequence: i64,
    pub channel: AudioChannel,
    #[ts(type = "number")]
    pub start_ms: i64,
    #[ts(type = "number")]
    pub end_ms: i64,
    pub text: String,
    /// Set by Diarization in M3. Until then the channel carries attribution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub speaker_id: Option<String>,
    /// Why `speaker_id` is what it is (ADR-0008's visible match
    /// attribution). Absent on segments written before Diarization ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub attribution: Option<Attribution>,
}

/// Which capture leg a segment came from. The mic channel is where the
/// Operator is (ADR-0029 as amended).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum AudioChannel {
    Mic,
    System,
}

impl AudioChannel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mic => "mic",
            Self::System => "system",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "mic" => Some(Self::Mic),
            "system" => Some(Self::System),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MeetingStartParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub detected_app: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MeetingStopParams {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MeetingResponse {
    pub meeting: Meeting,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MeetingListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub offset: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MeetingListResponse {
    pub meetings: Vec<Meeting>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MeetingGetParams {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MeetingDetailResponse {
    pub meeting: Meeting,
    pub segments: Vec<TranscriptSegment>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MeetingRetitleParams {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MeetingDeleteParams {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MeetingDeleteResponse {
    pub deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MeetingExportResponse {
    pub markdown: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub mirror_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HistorySearchParams {
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HistorySearchResponse {
    pub results: Vec<SearchResult>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SearchResult {
    pub meeting: Meeting,
    /// The matching text with the hit in context.
    pub snippet: String,
}

/// What happened to a Meeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum MeetingChangeKind {
    Started,
    Updated,
    Stopped,
    Deleted,
}

// ------------------------------------------------------------------ Settings

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SettingsGetParams {}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SettingsSetParams {
    /// Records that the Operator acknowledged the Briefing. Once true it is
    /// never set back to false by a Client: un-acknowledging consent is not
    /// a thing that happens, and allowing it would make the pre-capture
    /// invariant a toggle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub briefing_acknowledged: Option<bool>,
    /// Registration only — this changes the *next* login and leaves a
    /// running Core alone (story 9c).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub launch_at_login: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub auto_record: Option<bool>,
    /// Which Han script Mandarin is written in. See [`ChineseScript`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub chinese_script: Option<ChineseScript>,
}

/// Which Han script to write Mandarin in.
///
/// Transcription stays on automatic language detection, because meetings
/// code-switch (story 7) — so the script the model returns is whatever its
/// training data favoured, not what the speaker would have written. Left
/// alone it is frequently Traditional even for a Simplified speaker, which
/// the dogfood run measured. This is the Operator saying which one their
/// record should use; it is an orthographic choice, not a translation, and
/// the words are identical either way.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ChineseScript {
    /// The default: more people read it than any other Han script.
    #[default]
    Simplified,
    Traditional,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SettingsResponse {
    pub briefing_acknowledged: bool,
    pub launch_at_login: bool,
    pub auto_record: bool,
    pub chinese_script: ChineseScript,
    /// Where the login-item registration lives, so the Operator can see and
    /// remove it by hand.
    pub launch_at_login_location: String,
    /// True when the setting and the actual registration disagree — for
    /// example after the Operator deleted the plist themselves.
    pub launch_at_login_registered: bool,
}

// ----------------------------------------------------------------- Watchlist

/// What a Watchlist row matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum WatchlistKind {
    /// One application, by bundle id or executable name.
    Process,
    /// Any browser holding a hot microphone — one row rather than a row per
    /// site, so Google Meet and the web variants of the desktop apps are
    /// covered without knowing any URLs (ADR-0030).
    BrowserMeetings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WatchlistEntry {
    pub id: String,
    pub name: String,
    pub kind: WatchlistKind,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WatchlistGetParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WatchlistAddParams {
    pub id: String,
    /// Omitted for a suggested entry, whose name the Core already knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kind: Option<WatchlistKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WatchlistRemoveParams {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WatchlistResponse {
    /// What is watched, in a stable order.
    pub entries: Vec<WatchlistEntry>,
    /// Known call apps offered but not watched — WeChat ships here rather
    /// than on the list, because default-recording personal calls is the
    /// posture ADR-0030 declined.
    pub suggestions: Vec<WatchlistEntry>,
}

// ---------------------------------------------------------------- Transcript

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TranscriptSubscribeParams {
    /// Which Meeting to follow. Omit for the one currently recording.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub meeting_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TranscriptSnapshotResponse {
    /// None when nothing is recording and no Meeting was named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub meeting: Option<Meeting>,
    /// Everything transcribed so far. Live segments arrive as notifications
    /// from the moment this response is produced.
    pub segments: Vec<TranscriptSegment>,
    pub subscribed: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TranscriptUnsubscribeParams {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TranscriptUnsubscribeResponse {
    pub subscribed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TranscriptSegmentAddedParams {
    pub meeting_id: String,
    pub segment: TranscriptSegment,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TranscriptCaptionsDroppedParams {
    pub meeting_id: String,
    #[ts(type = "number")]
    pub dropped: u32,
}

// -------------------------------------------------------------------- Models

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelsStatusParams {}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelsFetchParams {
    /// Fetch one model by key; omit to fetch everything required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelsStatusResponse {
    pub models: Vec<ModelState>,
    /// True when everything required is present and verified — the gate the
    /// tray's not-ready state and any capture attempt read.
    pub ready: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelState {
    pub key: String,
    pub display_name: String,
    pub state: ModelAvailability,
    pub required: bool,
    #[ts(type = "number")]
    pub total_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub bytes_on_disk: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub path: Option<String>,
    /// Why a model is corrupted, when it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ModelAvailability {
    Missing,
    Partial,
    Corrupted,
    Ready,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MeetingChangedParams {
    pub kind: MeetingChangeKind,
    pub meeting_id: String,
    /// Absent for a deletion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub meeting: Option<Meeting>,
}

/// Convenience: the id type re-exported for clients building requests.
pub type ClientRequestId = RequestId;

/// A Speaker, as the Voice Registry shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Speaker {
    pub id: String,
    /// Absent until the Operator names it. Clients render a numbered
    /// pseudonym ("Speaker 3"); it is deliberately not stored, because a
    /// stored pseudonym looks like a name someone chose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub display_name: Option<String>,
    /// The Operator's own Speaker, displayed "You" (ADR-0029 as amended).
    pub is_operator: bool,
    /// Whether the app can still recognize this voice. False after a
    /// Voiceprint deletion, with the Speaker and the record intact.
    pub has_voiceprint: bool,
    /// Set when the Operator named this Speaker: naming is confirmation
    /// (ADR-0008 as amended), and a confirmed Voiceprint outranks an
    /// unconfirmed one when matching.
    pub confirmed: bool,
    /// Which model produced the stored Voiceprint. Present so a Registry can
    /// explain why an upgrade re-embedded, rather than recognition silently
    /// changing quality.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub voiceprint_model: Option<String>,
    #[ts(type = "number")]
    pub meetings_seen_in: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub first_seen_at: Option<String>,
    pub created_at: String,
}

/// Why a segment is attributed to whom it is.
///
/// ADR-0008 counts visible match attribution among the legibility surfaces
/// it made mandatory in exchange for storing biometrics. An Operator who
/// cannot ask why has no way to know whether to correct it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Attribution {
    /// Matched an existing Voiceprint.
    Voiceprint,
    /// Clustered within this Meeting and matched nobody in History.
    Clustered,
    /// The mic-channel prior did the work (ADR-0029 as amended).
    Channel,
    /// The Operator said so, and that outranks everything above.
    Operator,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpeakerListParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpeakerListResponse {
    pub speakers: Vec<Speaker>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpeakerGetParams {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpeakerResponse {
    pub speaker: Speaker,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpeakerDetailResponse {
    pub speaker: Speaker,
    /// Names the calendar knew about for Meetings this Speaker appears in,
    /// offered as candidates. **Suggestions only** — an invitation is
    /// evidence about who was invited, never about who spoke, and applying
    /// one automatically would invent attribution.
    pub name_suggestions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpeakerRenameParams {
    pub id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpeakerChangedParams {
    pub speaker: Speaker,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TranscriptReassignParams {
    pub segment_id: String,
    pub speaker_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TranscriptReassignResponse {
    pub segment: TranscriptSegment,
}

/// What a diarization job is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DiarizeState {
    Idle,
    Running,
    /// Ran, and could not: a missing or unloadable model. The Transcript
    /// stays unattributed and says so rather than the Meeting failing.
    Unavailable,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiarizeStatusParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiarizeStatusResponse {
    pub state: DiarizeState,
    /// The Meeting being diarized, when one is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub meeting_id: Option<String>,
    #[ts(type = "number")]
    pub done_ms: i64,
    #[ts(type = "number")]
    pub total_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiarizeCancelParams {
    pub meeting_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiarizeProgressParams {
    pub meeting_id: String,
    pub state: DiarizeState,
    #[ts(type = "number")]
    pub done_ms: i64,
    #[ts(type = "number")]
    pub total_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_method_in_the_table_is_wired_to_a_params_type() {
        // A method is either decodable from empty params (all fields
        // defaulted) or rejects them as invalid. What it must never do is
        // come back unknown — that would mean the table names a method the
        // decoder cannot reach.
        for method in ClientRequest::METHODS {
            match ClientRequest::from_wire(method, None) {
                Ok(request) => assert_eq!(&request.method(), method),
                Err(DecodeError::InvalidParams { method: named, .. }) => {
                    assert_eq!(&named, method)
                }
                Err(DecodeError::UnknownMethod(unknown)) => {
                    panic!("{unknown} is in METHODS but the decoder does not know it")
                }
            }
        }
    }

    #[test]
    fn every_method_declares_a_response_type() {
        assert_eq!(
            ClientRequest::RESPONSE_TYPES.len(),
            ClientRequest::METHODS.len()
        );
        for (method, response) in ClientRequest::RESPONSE_TYPES {
            assert!(
                !response.is_empty(),
                "{method} must declare a response type"
            );
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
