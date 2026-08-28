//! Wire types for the EverTranscript Core protocol (ADR-0028).
//!
//! The Core is the only writer of the record; every surface — the Electron
//! Client, the CLI — reaches it through the types in this crate, over
//! newline-delimited JSON on a unix socket (macOS) or named pipe (Windows).
//!
//! TypeScript bindings are generated from these types by `cargo test`
//! (ts-rs) into `bindings/`, and the JSON Schema into `schema/`. Both are
//! committed and drift-checked in CI, so a protocol change that forgets the
//! Client is a failing build rather than a runtime surprise.

pub mod protocol;
pub mod rpc;

pub use protocol::*;
pub use rpc::*;

/// Wire-protocol version. See `ServerInfo::protocol_version`.
pub const PROTOCOL_VERSION: u32 = 1;

/// The name the Core reports in `initialize`.
pub const SERVER_NAME: &str = "evertranscript-core";

/// This build's version, from Cargo.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
