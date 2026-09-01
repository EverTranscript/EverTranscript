//! Shared by the integration tests that drive a Core over its transport.
//!
//! The Core has spoken two transports since M2 — a Unix socket and a Windows
//! named pipe, chosen in `lib.rs` — but the tests that drive it were written
//! when there was only one, and they carried `#![cfg(unix)]` ever since. The
//! gate was never about the Core; it was about these harnesses hard-coding a
//! filesystem path for the endpoint. This is the two lines that were missing
//! (DECISIONS Q54).
//!
//! `bind` takes `&Path` on unix and `&str` on Windows, and so does
//! `CoreClient::connect_to`. A single alias lets one harness satisfy both,
//! because `&PathBuf` and `&String` deref to exactly what each wants.

/// What this platform's transport is addressed by.
#[cfg(unix)]
pub type Endpoint = std::path::PathBuf;
#[cfg(windows)]
pub type Endpoint = String;

/// A private endpoint for one test.
///
/// On unix a file inside the test's own temporary directory, kept short
/// because socket paths are length-limited. On Windows a name in the
/// machine-wide pipe namespace, which has no directory to be scoped by — so
/// it carries a uuid instead, and two tests running at once cannot collide.
#[cfg(unix)]
pub fn endpoint(dir: &std::path::Path) -> Endpoint {
    dir.join("s")
}

#[cfg(windows)]
pub fn endpoint(_dir: &std::path::Path) -> Endpoint {
    format!(r"\\.\pipe\evertranscript-test-{}", uuid::Uuid::now_v7())
}
