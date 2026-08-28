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
    format!(r"\\.\pipe\evertranscript-{user}")
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
