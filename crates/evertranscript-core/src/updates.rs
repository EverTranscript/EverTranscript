//! The update feed: Sanctioned Traffic entry one.
//!
//! ADR-0016 chose direct download plus in-app updates precisely so shipping
//! never waits on an app store's opinion of system-audio capture. ADR-0034
//! then made the check one of exactly three things this product may ever say
//! on the wire, **disableable in Settings** — and the guarantee test's final
//! form depends on that switch: "with updates off and models downloaded,
//! literally zero".
//!
//! The check itself is the Client's. electron-updater replaces the whole
//! bundle, Core included, and reads the switch from the Core's settings
//! (`clients/electron/src/main/updates.ts`). The Core keeps only the host, so
//! the trust surface can name it (DECISIONS Q306).

/// Where the update feed lives.
///
/// Named here rather than assembled at the call site so the trust surface
/// can show the exact host an Operator would see in a firewall log.
pub const UPDATE_FEED_HOST: &str = "https://github.com/EverTranscript/EverTranscript";

/// Where GitHub serves a release's files: the feed file every check reads,
/// and the update itself when the Operator presses Download. Release links
/// on the feed host redirect here, so a firewall log shows both (Q335).
pub const UPDATE_FILES_HOST: &str = "https://release-assets.githubusercontent.com";

/// Whether any process is running the executable at `path` (DECISIONS Q333).
///
/// The Windows installer stops every process under the install folder, this
/// Core included, and nothing bounds how long after the Client quits it gets
/// to that. So an update's hold lasts for as long as its installer runs, and
/// this is how the Core knows. Paths are compared as the file system resolves
/// them, so neither case nor an 8.3 name decides it.
///
/// If the processes cannot be listed, the answer is yes: a hold that stands
/// too long delays a recording, and one that ends too early lets the
/// installer stop it.
#[cfg(windows)]
pub fn installer_running(path: &std::path::Path) -> bool {
    use windows::Win32::System::ProcessStatus::EnumProcesses;

    // A path that resolves to nothing has nothing running from it.
    let Ok(wanted) = std::fs::canonicalize(path) else {
        return false;
    };
    let Some(leaf) = wanted.file_name().map(|leaf| leaf.to_ascii_lowercase()) else {
        return false;
    };
    let mut pids = vec![0u32; 1024];
    loop {
        let size = std::mem::size_of_val(pids.as_slice()) as u32;
        let mut needed = 0u32;
        if let Err(error) = unsafe { EnumProcesses(pids.as_mut_ptr(), size, &mut needed) } {
            tracing::warn!(%error, "could not list processes; the update hold stands");
            return true;
        }
        // A full buffer may have cut the list short.
        if needed < size {
            pids.truncate(needed as usize / std::mem::size_of::<u32>());
            break;
        }
        pids.resize(pids.len() * 2, 0);
    }
    pids.into_iter().filter_map(image_of).any(|image| {
        image.file_name().map(|name| name.to_ascii_lowercase()) == Some(leaf.clone())
            && std::fs::canonicalize(&image).is_ok_and(|image| image == wanted)
    })
}

/// The macOS update swaps the bundle and stops no process, so no installer
/// there can stop this Core.
#[cfg(not(windows))]
pub fn installer_running(_path: &std::path::Path) -> bool {
    false
}

/// The full path of the executable behind a process id.
#[cfg(windows)]
fn image_of(pid: u32) -> Option<std::path::PathBuf> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::OpenProcess;
    use windows::Win32::System::Threading::PROCESS_NAME_WIN32;
    use windows::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION;
    use windows::Win32::System::Threading::QueryFullProcessImageNameW;
    use windows::core::PWSTR;

    unsafe {
        // The idle and system processes, and anything this user may not
        // query, will not open; none of them is an installer this Client ran.
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 32768];
        let mut size = buffer.len() as u32;
        let read = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(handle);
        read.ok()?;
        Some(String::from_utf16_lossy(&buffer[..size as usize]).into())
    }
}

#[cfg(test)]
mod tests {
    use super::installer_running;

    #[test]
    fn a_path_nothing_runs_from_is_not_running() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(!installer_running(
            &dir.path().join("EverTranscript-Setup-9.9.9.exe")
        ));
    }

    /// The one process certain to be running: this test.
    #[cfg(windows)]
    #[test]
    fn a_running_executable_is_found_under_any_spelling_of_its_path() {
        let me = std::env::current_exe().expect("current exe");
        assert!(installer_running(&me));
        let shouted = std::path::PathBuf::from(me.to_string_lossy().to_uppercase());
        assert!(installer_running(&shouted), "case must not decide it");
    }

    /// A stand-in installer at a downloaded installer's kind of path: found
    /// while it runs, under its 8.3 name too, and not once it has exited.
    #[cfg(windows)]
    #[test]
    fn an_installer_is_found_while_it_runs_and_not_after() {
        use std::process::{Command, Stdio};

        let dir = tempfile::tempdir().expect("tempdir");
        let installer = dir.path().join("EverTranscript-Setup-9.9.9.exe");
        // findstr reads its input until it closes, so it runs exactly as long
        // as this test holds that open: no network, no window, no timer.
        let system = std::env::var_os("SystemRoot").expect("SystemRoot");
        std::fs::copy(
            std::path::Path::new(&system).join(r"System32\findstr.exe"),
            &installer,
        )
        .expect("copy findstr");
        assert!(!installer_running(&installer), "nothing runs from it yet");

        let mut child = Command::new(&installer)
            .arg("x")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start the stand-in installer");
        assert!(installer_running(&installer), "a running installer holds");
        match short_name(&installer) {
            Some(short) => {
                eprintln!("8.3 name exercised: {}", short.display());
                assert!(installer_running(&short), "an 8.3 name must not decide it");
            }
            None => eprintln!("this volume has no 8.3 names; that spelling is not exercised"),
        }

        drop(child.stdin.take());
        child.wait().expect("the stand-in installer exits");
        // The Core never holds a handle to the installer; neither may the test.
        drop(child);
        assert!(
            !installer_running(&installer),
            "an installer that has exited holds nothing"
        );
    }

    /// The path's 8.3 spelling, when the volume keeps one and it differs.
    #[cfg(windows)]
    fn short_name(path: &std::path::Path) -> Option<std::path::PathBuf> {
        use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
        use windows::Win32::Storage::FileSystem::GetShortPathNameW;
        use windows::core::PCWSTR;

        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut buffer = vec![0u16; 32768];
        let length =
            unsafe { GetShortPathNameW(PCWSTR(wide.as_ptr()), Some(buffer.as_mut_slice())) }
                as usize;
        if length == 0 || length >= buffer.len() {
            return None;
        }
        let short = std::path::PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length]));
        (short.file_name() != path.file_name()).then_some(short)
    }
}
