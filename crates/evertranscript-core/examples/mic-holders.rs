//! What is holding the microphone, before the Watchlist has an opinion.
//!
//! The Core logs an app only once a Watchlist row has matched it
//! (`driver.rs`, "Auto-Record started a Meeting"). So the one case this
//! milestone keeps losing to — an app recording under a name its row does
//! not hold — is exactly the case that logs nothing at all. Four times the
//! answer has been a name nobody had read off a machine: Safari's
//! `com.apple.WebKit.GPU`, Teams' `com.microsoft.teams2.modulehost`, Arc's
//! one-letter case difference, and a VooV executable wrong in both halves.
//!
//! `Get-Process` cannot settle it either: it says a process exists, not that
//! the process owns the capture session. So this prints the session's own
//! account of itself — the raw executable name, the session identifier's
//! path, and what `responsible_app` then makes of it — and lets the two be
//! compared.
//!
//! It reads. It records nothing and changes nothing.
//!
//! ```text
//! cargo run -p evertranscript-core --example mic-holders
//! ```

#[cfg(not(windows))]
fn main() {
    eprintln!("mic-holders probes the WASAPI capture endpoint; this is not Windows.");
}

#[cfg(windows)]
fn main() {
    probe::run();
}

#[cfg(windows)]
mod probe {
    use evertranscript_core::detect::AppIdentity;
    use evertranscript_core::detect::watchlist::Watchlist;
    use evertranscript_core::detect::watchlist::known_browsers;
    use evertranscript_core::detect::watchlist::responsible_app;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Foundation::GetLastError;
    use windows::Win32::Media::Audio::AudioSessionStateActive;
    use windows::Win32::Media::Audio::AudioSessionStateExpired;
    use windows::Win32::Media::Audio::AudioSessionStateInactive;
    use windows::Win32::Media::Audio::DEVICE_STATE_ACTIVE;
    use windows::Win32::Media::Audio::IAudioSessionControl2;
    use windows::Win32::Media::Audio::IAudioSessionManager2;
    use windows::Win32::Media::Audio::IMMDevice;
    use windows::Win32::Media::Audio::IMMDeviceEnumerator;
    use windows::Win32::Media::Audio::MMDeviceEnumerator;
    use windows::Win32::Media::Audio::eCapture;
    use windows::Win32::Media::Audio::eMultimedia;
    use windows::Win32::System::Com::CLSCTX_ALL;
    use windows::Win32::System::Com::COINIT_MULTITHREADED;
    use windows::Win32::System::Com::CoCreateInstance;
    use windows::Win32::System::Com::CoInitializeEx;
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::System::ProcessStatus::GetModuleBaseNameW;
    use windows::Win32::System::Threading::OpenProcess;
    use windows::Win32::System::Threading::PROCESS_NAME_WIN32;
    use windows::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION;
    use windows::Win32::System::Threading::QueryFullProcessImageNameW;
    use windows::core::Interface;
    use windows::core::PWSTR;

    /// The name `detect::windows::executable_name` would report, and why not
    /// when it cannot. A session whose process will not open is invisible to
    /// the detector, which is a finding rather than a gap in this probe.
    fn module_base_name(pid: u32) -> Result<String, String> {
        unsafe {
            let handle = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                Ok(handle) => handle,
                Err(error) => return Err(format!("OpenProcess failed: {error}")),
            };
            let mut buffer = [0u16; 260];
            let written = GetModuleBaseNameW(handle, None, &mut buffer);
            let last = GetLastError();
            let _ = CloseHandle(handle);
            if written == 0 {
                return Err(format!(
                    "GetModuleBaseNameW returned nothing (last error {last:?})"
                ));
            }
            Ok(String::from_utf16_lossy(&buffer[..written as usize]).to_lowercase())
        }
    }

    /// The same question asked the way `PROCESS_QUERY_LIMITED_INFORMATION`
    /// can actually answer it. `GetModuleBaseNameW` is a PSAPI call over the
    /// module list and documents a need for `PROCESS_QUERY_INFORMATION` and
    /// `PROCESS_VM_READ`; this one is documented against the limited right
    /// the detector opens with. Printed beside it so the difference is
    /// evidence rather than assertion.
    fn full_image_name(pid: u32) -> Result<String, String> {
        unsafe {
            let handle = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                Ok(handle) => handle,
                Err(error) => return Err(format!("OpenProcess failed: {error}")),
            };
            let mut buffer = [0u16; 32768];
            let mut size = buffer.len() as u32;
            let outcome = QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut size,
            );
            let last = GetLastError();
            let _ = CloseHandle(handle);
            if outcome.is_err() {
                return Err(format!(
                    "QueryFullProcessImageNameW failed (last error {last:?})"
                ));
            }
            Ok(String::from_utf16_lossy(&buffer[..size as usize]))
        }
    }

    /// Reads one of COM's out-parameter strings and frees it.
    unsafe fn take_pwstr(text: PWSTR) -> String {
        unsafe {
            if text.is_null() {
                return String::new();
            }
            let owned = text.to_string().unwrap_or_default();
            CoTaskMemFree(Some(text.0 as *const _));
            owned
        }
    }

    /// Every active capture endpoint, the default one marked.
    ///
    /// The detector only ever asks the default (`GetDefaultAudioEndpoint`),
    /// so an app recording from a second microphone would be invisible to
    /// it. Printing all of them is how that stays visible here.
    fn capture_endpoints(enumerator: &IMMDeviceEnumerator) -> Vec<(IMMDevice, String, bool)> {
        let mut endpoints = Vec::new();
        unsafe {
            let default_id = enumerator
                .GetDefaultAudioEndpoint(eCapture, eMultimedia)
                .ok()
                .and_then(|device| device.GetId().ok())
                .map(|id| take_pwstr(id))
                .unwrap_or_default();

            let Ok(collection) = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)
            else {
                return endpoints;
            };
            let Ok(count) = collection.GetCount() else {
                return endpoints;
            };
            for index in 0..count {
                let Ok(device) = collection.Item(index) else {
                    continue;
                };
                let id = device
                    .GetId()
                    .ok()
                    .map(|id| take_pwstr(id))
                    .unwrap_or_default();
                let is_default = !default_id.is_empty() && id == default_id;
                endpoints.push((device, id, is_default));
            }
        }
        endpoints
    }

    fn state_label(control: &IAudioSessionControl2) -> &'static str {
        match unsafe { control.GetState() } {
            Ok(state) if state == AudioSessionStateActive => "ACTIVE",
            Ok(state) if state == AudioSessionStateInactive => "inactive",
            Ok(state) if state == AudioSessionStateExpired => "expired",
            Ok(_) => "unknown",
            Err(_) => "unreadable",
        }
    }

    /// `--pid N` asks the two name questions about one process and stops.
    /// The audio sessions are the interesting case, but proving the
    /// difference is about *access rights* rather than about one vendor's
    /// sandbox needs a process the caller plainly owns.
    fn describe_one(pid: u32) {
        println!("pid {pid}");
        match module_base_name(pid) {
            Ok(name) => println!("  GetModuleBaseNameW        {name}"),
            Err(why) => println!("  GetModuleBaseNameW        UNREADABLE — {why}"),
        }
        match full_image_name(pid) {
            Ok(path) => println!("  QueryFullProcessImageName {path}"),
            Err(why) => println!("  QueryFullProcessImageName UNREADABLE — {why}"),
        }
    }

    pub fn run() {
        let mut args = std::env::args().skip(1);
        if args.next().as_deref() == Some("--pid") {
            match args.next().and_then(|pid| pid.parse::<u32>().ok()) {
                Some(pid) => describe_one(pid),
                None => eprintln!("--pid wants a process id"),
            }
            return;
        }

        let watchlist = Watchlist::shipped();
        let browsers = known_browsers();

        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let Ok(enumerator) =
                CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
            else {
                eprintln!("no device enumerator; nothing can be said");
                return;
            };

            let endpoints = capture_endpoints(&enumerator);
            if endpoints.is_empty() {
                println!("no active capture endpoints — this machine has no microphone");
                return;
            }

            for (device, id, is_default) in &endpoints {
                let marker = if *is_default {
                    "  [DEFAULT — the only one the detector reads]"
                } else {
                    "  [not the default; the detector cannot see this one]"
                };
                println!("\nendpoint {id}{marker}");

                let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
                    println!("  (its session manager would not activate)");
                    continue;
                };
                let Ok(sessions) = manager.GetSessionEnumerator() else {
                    println!("  (it has no session enumerator)");
                    continue;
                };
                let Ok(count) = sessions.GetCount() else {
                    continue;
                };
                if count == 0 {
                    println!("  (no sessions)");
                    continue;
                }

                for index in 0..count {
                    let Ok(control) = sessions.GetSession(index) else {
                        continue;
                    };
                    let Ok(control) = control.cast::<IAudioSessionControl2>() else {
                        continue;
                    };
                    let state = state_label(&control);
                    let pid = control.GetProcessId().unwrap_or(0);
                    let session_id = control
                        .GetSessionIdentifier()
                        .ok()
                        .map(|id| take_pwstr(id))
                        .unwrap_or_default();

                    println!("  session {index}: state={state} pid={pid}");
                    // The call the detector used to make, kept only as the
                    // side-by-side. It answers `ERROR_ACCESS_DENIED` for
                    // effectively every process, so nothing may be derived
                    // from it — deriving the verdict from it is how the
                    // first draft of this probe contrived to never print the
                    // one line it exists to print.
                    match module_base_name(pid) {
                        Ok(name) => println!("    (old) base name  {name}"),
                        Err(why) => println!("    (old) base name  UNREADABLE — {why}"),
                    }
                    match full_image_name(pid) {
                        Ok(path) => {
                            let name = path.rsplit('\\').next().unwrap_or(&path).to_lowercase();
                            let mapped = responsible_app(&name);
                            let watched = watchlist.watches(&AppIdentity::bare(&mapped));
                            let is_browser = browsers.iter().any(|b| {
                                b.eq_ignore_ascii_case(&mapped) || b.eq_ignore_ascii_case(&name)
                            });
                            println!("    executable   {name}");
                            println!("    responsible  {mapped}");
                            println!(
                                "    shipped Watchlist watches it: {}{}",
                                if watched { "YES" } else { "NO" },
                                if is_browser {
                                    "  (a known browser)"
                                } else {
                                    ""
                                }
                            );
                            println!("    full path    {path}");
                        }
                        // The detector drops these silently; say so loudly.
                        Err(why) => println!("    executable   UNREADABLE — {why}"),
                    }
                    if !session_id.is_empty() {
                        println!("    session id   {session_id}");
                    }
                }
            }
        }
    }
}
