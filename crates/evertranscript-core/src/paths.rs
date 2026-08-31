//! Where the Core keeps things.
//!
//! Two distinct roots, and the distinction is load-bearing (ADR-0035):
//!
//! - The **History folder** (`~/Documents/EverTranscript` by default) is the
//!   Operator's portable unit: Mirrors at the top level, the machine store in
//!   a hidden `.data/`. Copying it moves the record *and* recognition.
//! - **Application Support** holds what is re-creatable — models, caches,
//!   logs, the runtime socket. Never part of the portable unit.

use std::path::Path;
use std::path::PathBuf;

/// Environment override for the History folder, used by tests and by
/// Operators who relocate it.
pub const HISTORY_DIR_ENV: &str = "EVERTRANSCRIPT_HISTORY_DIR";

/// Environment override for the runtime directory (socket, lock).
pub const RUNTIME_DIR_ENV: &str = "EVERTRANSCRIPT_RUNTIME_DIR";

/// Overrides Application Support — models, settings, logs.
///
/// **Added because its absence made two guarantee tests prove less than
/// they claimed.** Both set `EVERTRANSCRIPT_MODELS_DIR`, which nothing read;
/// they copied models into a directory the Core never looked at and then ran
/// against whatever the developer's machine happened to have. On a runner
/// with no models they would have exercised a Core that could not diarize or
/// summarize at all, found no network traffic, and passed — which is a true
/// sentence about nothing.
///
/// It also isolates `settings.json`, and that half is worse: without it a
/// test that chose a Summary Backend wrote to the real machine's settings,
/// and a "fresh install" on any machine that has ever run this product
/// inherits its acknowledgment — so the pre-capture invariant appears to be
/// violated when it is only being read from the wrong file.
pub const APP_SUPPORT_DIR_ENV: &str = "EVERTRANSCRIPT_APP_SUPPORT_DIR";

/// The hidden machine store inside the History folder.
pub const DATA_DIR_NAME: &str = ".data";

/// The Operator-visible History folder.
pub fn history_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(HISTORY_DIR_ENV) {
        return PathBuf::from(dir);
    }
    dirs::document_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("EverTranscript")
}

/// The hidden machine store: database, WAL, per-Meeting audio.
pub fn data_dir() -> PathBuf {
    history_dir().join(DATA_DIR_NAME)
}

/// Per-Meeting audio files.
pub fn audio_dir() -> PathBuf {
    data_dir().join("audio")
}

/// The SQLite database — the record's source of truth.
pub fn database_path() -> PathBuf {
    data_dir().join("EverTranscript.db")
}

/// Re-creatable state: models, caches, logs.
pub fn app_support_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(APP_SUPPORT_DIR_ENV) {
        return PathBuf::from(dir);
    }
    dirs::data_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("EverTranscript")
}

/// Downloaded models (never part of the portable unit).
pub fn models_dir() -> PathBuf {
    app_support_dir().join("models")
}

/// Runtime directory for the socket and startup lock.
pub fn runtime_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(RUNTIME_DIR_ENV) {
        return PathBuf::from(dir);
    }
    app_support_dir().join("run")
}

/// The unix socket the Core listens on. Unix only; Windows uses a named pipe.
#[cfg(unix)]
pub fn socket_path() -> PathBuf {
    runtime_dir().join("evertranscript.sock")
}

/// The named pipe the Core listens on. Windows only.
#[cfg(windows)]
pub fn pipe_name() -> String {
    // Namespaced per user so two logged-in accounts never collide.
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "default".to_string());

    // And per runtime directory when one is named, which is what gives
    // Windows the isolation unix has had all along: there the socket *is* a
    // path inside that directory, so two Cores with different runtime
    // directories cannot collide. A single global pipe meant they always
    // did — the second Core failed to bind, `status` was answered by the
    // first, and a test then blamed its own History folder for being empty.
    pipe_name_for(std::env::var_os(RUNTIME_DIR_ENV).as_deref(), &user)
}

/// The derivation, split out so it can be tested on any platform — a
/// regression here is invisible until two Cores fight over one pipe.
///
/// Genuinely unused away from Windows, where there is no pipe to name; the
/// tests still exercise it everywhere so the property is not asserted only
/// on the platform that cannot easily run them.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn pipe_name_for(runtime_dir: Option<&std::ffi::OsStr>, user: &str) -> String {
    match runtime_dir {
        Some(dir) => {
            // A path cannot go in a pipe name; a stable digest of it can.
            let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
            for byte in dir.as_encoded_bytes() {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x1000_0000_01b3);
            }
            format!(r"\\.\pipe\evertranscript-{user}-{hash:016x}")
        }
        None => format!(r"\\.\pipe\evertranscript-{user}"),
    }
}

/// Serializes competing Core startups (ported lock discipline, ADR-0028).
pub fn startup_lock_path() -> PathBuf {
    runtime_dir().join("startup.lock")
}

/// A human-readable description of the listen address, for `status` and logs.
pub fn listen_address_display() -> String {
    #[cfg(unix)]
    {
        socket_path().display().to_string()
    }
    #[cfg(windows)]
    {
        pipe_name()
    }
}

/// True when a History folder looks like an incomplete copy: Mirrors are
/// present but the hidden machine store is not (ADR-0035). A caller that
/// finds this must warn rather than silently create a fresh store, because
/// the Operator believes they copied their History.
pub fn detect_incomplete_copy(history: &Path) -> bool {
    if !history.is_dir() || history.join(DATA_DIR_NAME).exists() {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(history) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry
            .path()
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
    })
}

/// Creates the History folder and its hidden machine store.
///
/// On Windows the `.`-prefix hides nothing, so the directory also gets the
/// hidden attribute — without it the "reads as meeting notes" property that
/// motivated the hidden store (ADR-0035) is macOS-only.
pub fn ensure_history_layout(history: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(history)?;
    let data = history.join(DATA_DIR_NAME);
    let created = !data.exists();
    std::fs::create_dir_all(data.join("audio"))?;
    if created {
        set_hidden(&data)?;
    }
    Ok(())
}

#[cfg(windows)]
fn set_hidden(path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 path that outlives the call.
    let result = unsafe { SetFileAttributesW(wide.as_ptr(), FILE_ATTRIBUTE_HIDDEN) };
    if result == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetFileAttributesW(path: *const u16, attributes: u32) -> i32;
}

#[cfg(not(windows))]
fn set_hidden(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_named_runtime_directory_gives_windows_its_own_pipe() {
        // The isolation unix gets for free from the socket being a path.
        // Asserted on both platforms because the derivation is shared and a
        // regression here is invisible until two Cores fight over one pipe.
        let a = pipe_name_for(Some("/tmp/one".as_ref()), "frank");
        let b = pipe_name_for(Some("/tmp/two".as_ref()), "frank");
        let none = pipe_name_for(None, "frank");
        assert_ne!(a, b, "different runtime directories must not share a pipe");
        assert_ne!(a, none, "a named directory must not reuse the default pipe");
        assert_eq!(
            a,
            pipe_name_for(Some("/tmp/one".as_ref()), "frank"),
            "the same directory must always produce the same pipe"
        );
        assert!(!a.contains('/'), "a path cannot appear in a pipe name: {a}");
    }

    #[test]
    fn a_fresh_folder_is_not_an_incomplete_copy() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(!detect_incomplete_copy(dir.path()));
    }

    #[test]
    fn mirrors_without_the_machine_store_are_an_incomplete_copy() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("2026-08-27-zoom-a3f8c21b.md"), "# Meeting")
            .expect("write mirror");
        assert!(detect_incomplete_copy(dir.path()));
    }

    #[test]
    fn a_complete_copy_is_not_flagged() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("2026-08-27-zoom-a3f8c21b.md"), "# Meeting")
            .expect("write mirror");
        ensure_history_layout(dir.path()).expect("layout");
        assert!(!detect_incomplete_copy(dir.path()));
    }

    #[test]
    fn the_layout_creates_the_hidden_store_and_audio_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        ensure_history_layout(dir.path()).expect("layout");
        assert!(dir.path().join(DATA_DIR_NAME).is_dir());
        assert!(dir.path().join(DATA_DIR_NAME).join("audio").is_dir());
    }
}
