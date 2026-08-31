//! Windows detection: who is holding the microphone.
//!
//! The same shape as [`super::macos`], through a different API. WASAPI
//! exposes the capture endpoint's audio sessions; each session knows its
//! process and whether it is active, which is per-process microphone
//! attribution without a permission prompt — the one place Windows asks less
//! of the Operator than macOS does.
//!
//! ADR-0025 as amended makes this a ship gate rather than a follow-up, and
//! ADR-0030 is explicit that the column cannot be hollow: anarlog's Windows
//! detector is a no-op stub and is not prior art. The shipped prior art is
//! Granola's `mic_monitor_v2` — backoff, give-up, and an exe→app table —
//! which was not on the machine this was written on, so the retry shape here
//! is the same poll-and-debounce the macOS side uses rather than a port of
//! theirs.
//!
//! **Verification status, stated plainly.** Run on Windows 11 Pro 26200 on
//! 2026-08-31, and it did not work. It had never named a single process.
//! `executable_name` asked PSAPI's `GetModuleBaseNameW` for a name over a
//! handle opened with `PROCESS_QUERY_LIMITED_INFORMATION`, which that call
//! is not documented against; every call on every process returned
//! `ERROR_ACCESS_DENIED`, and a zero return is indistinguishable here from
//! "no such process". So `microphone_holders` answered "nobody" while Edge
//! plainly held the microphone, and no Watchlist row could ever have
//! matched — not the browsers, not `WINDOWS_EXECUTABLES`, none of it. The
//! whole platform was dark, on the platform ADR-0025 makes the ship gate.
//!
//! Typechecking is what hid it, and typechecking was all this file had: the
//! call was correct Rust against a real API and simply lacked a right. There
//! was no test, because there was nothing here that ran. There is one now,
//! and it asserts against a name read off the machine at runtime rather than
//! one written down.
//!
//! Since the fix, observed live: Edge holding the microphone starts and
//! stops a Meeting as `msedge.exe`, and two Cores with different runtime
//! dirs bind different pipes. The meeting apps in `WINDOWS_EXECUTABLES` are
//! **still unobserved** — none of them is installed on that machine.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Instant;

use anyhow::Result;
use tokio::sync::mpsc;
use tracing::debug;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Media::Audio::AudioSessionStateActive;
use windows::Win32::Media::Audio::DEVICE_STATE_ACTIVE;
use windows::Win32::Media::Audio::IAudioSessionControl2;
use windows::Win32::Media::Audio::IAudioSessionManager2;
use windows::Win32::Media::Audio::IMMDevice;
use windows::Win32::Media::Audio::IMMDeviceEnumerator;
use windows::Win32::Media::Audio::MMDeviceEnumerator;
use windows::Win32::Media::Audio::eCapture;
use windows::Win32::System::Com::CLSCTX_ALL;
use windows::Win32::System::Com::COINIT_MULTITHREADED;
use windows::Win32::System::Com::CoCreateInstance;
use windows::Win32::System::Com::CoInitializeEx;
use windows::Win32::System::Threading::OpenProcess;
use windows::Win32::System::Threading::PROCESS_NAME_WIN32;
use windows::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION;
use windows::Win32::System::Threading::QueryFullProcessImageNameW;
use windows::core::Interface;
use windows::core::PWSTR;

use super::AppIdentity;
use super::DetectionEvent;
use super::DetectionInstant;
use super::DetectionSource;
use super::watchlist::responsible_app;

/// See [`super::macos::POLL_MS`] — the same reasoning, the same number.
const POLL_MS: u64 = 500;
const RELEASE_DEBOUNCE_MS: u64 = 2_000;

/// The executable behind a process id, lowercased so the Watchlist can hold
/// one spelling.
///
/// `QueryFullProcessImageNameW`, and not PSAPI's `GetModuleBaseNameW`, on
/// which this shipped and never once succeeded. That call walks the target's
/// module list and documents a need for `PROCESS_QUERY_INFORMATION` and
/// `PROCESS_VM_READ`. The handle below carries only the limited query right,
/// which is what a detector watching other people's processes ought to ask
/// for and the one this API is documented against. So every call failed with
/// `ERROR_ACCESS_DENIED`, returned 0, and was read here as "no such process"
/// — for Edge, for Teams, and for this process itself. Nothing was ever
/// named, so no Watchlist row could ever match, and no test noticed because
/// nothing called it.
fn executable_name(pid: u32) -> Option<String> {
    unsafe {
        let handle = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(handle) => handle,
            Err(error) => {
                debug!(pid, %error, "a process holding the microphone would not open; it cannot be attributed");
                return None;
            }
        };
        // 32767 is NTFS's own ceiling on a path, so this cannot truncate.
        // Sized to make that true rather than to be generous: a short buffer
        // would fail the same silent way the access right did.
        let mut buffer = [0u16; 32768];
        let mut size = buffer.len() as u32;
        let read = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(handle);
        if let Err(error) = read {
            debug!(pid, %error, "a process holding the microphone would not name itself; it cannot be attributed");
            return None;
        }
        if size == 0 {
            debug!(
                pid,
                "a process holding the microphone named itself with an empty path"
            );
            return None;
        }
        // It answers with a full path; the Watchlist holds the leaf.
        leaf_lowercased(&String::from_utf16_lossy(&buffer[..size as usize]))
    }
}

/// The leaf of a Win32 path, in the spelling the Watchlist compares against.
///
/// Split out so the lowercasing has a test that can fail. Windows spells its
/// own paths in mixed case and treats them case-insensitively, and comparing
/// two spellings exactly is precisely what cost Arc — `Browser` against
/// `browser`, one letter, matching nothing (`DECISIONS.md` Q22).
fn leaf_lowercased(path: &str) -> Option<String> {
    let leaf = path.rsplit('\\').next().unwrap_or(path);
    if leaf.is_empty() {
        return None;
    }
    Some(leaf.to_lowercase())
}

/// Which apps are holding the microphone right now, as responsible apps.
///
/// **Every active capture endpoint, not the default one.** This asked
/// `GetDefaultAudioEndpoint(eCapture, eMultimedia)` until it was noticed that
/// Windows keeps a *separate* default per `ERole` — `eConsole`,
/// `eMultimedia`, `eCommunications` — and points communications software at
/// `eCommunications`. Meeting apps are communications software, and Windows
/// reassigns that role by itself when a headset appears, so the two roles
/// routinely disagree and the detector was liable to watch the endpoint the
/// meeting was not on. A headset was enough to cause it; a second microphone
/// was never required. Enumerating them all subsumes both cases and removes
/// the need to guess which role a given app chose.
///
/// Failures are per-device and never fatal. The bug this platform shipped
/// with was a call that failed and looked exactly like an idle machine, so a
/// single unreadable endpoint must not be able to blank the whole answer —
/// and a machine that offers no endpoints at all says so in the log rather
/// than answering "nobody" in silence.
pub fn microphone_holders() -> BTreeSet<String> {
    let mut holders = BTreeSet::new();
    unsafe {
        // Already-initialised is not an error here: the Core may have a COM
        // apartment from capture already.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let Ok(enumerator) =
            CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
        else {
            tracing::debug!("no device enumerator; reporting no microphone holders");
            return holders;
        };
        // A machine with no microphone answers "nobody", which is the
        // truthful answer rather than an error.
        let Ok(endpoints) = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE) else {
            tracing::debug!("could not enumerate capture endpoints");
            return holders;
        };
        let Ok(endpoint_count) = endpoints.GetCount() else {
            tracing::debug!("could not count capture endpoints");
            return holders;
        };
        if endpoint_count == 0 {
            tracing::debug!("no active capture endpoints on this machine");
            return holders;
        }

        for endpoint in 0..endpoint_count {
            let Ok(device): windows::core::Result<IMMDevice> = endpoints.Item(endpoint) else {
                continue;
            };
            let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
                continue;
            };
            let Ok(sessions) = manager.GetSessionEnumerator() else {
                continue;
            };
            let Ok(count) = sessions.GetCount() else {
                continue;
            };

            for index in 0..count {
                let Ok(control) = sessions.GetSession(index) else {
                    continue;
                };
                let Ok(control2) = control.cast::<IAudioSessionControl2>() else {
                    continue;
                };
                // Inactive and expired sessions linger; only an active one is
                // a live microphone.
                if control2.GetState() != Ok(AudioSessionStateActive) {
                    continue;
                }
                let Ok(pid) = control2.GetProcessId() else {
                    continue;
                };
                if let Some(name) = executable_name(pid) {
                    holders.insert(responsible_app(&name));
                }
            }
        }
    }
    holders
}

/// The live Windows DetectionSource.
pub struct WindowsDetectionSource {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Default for WindowsDetectionSource {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsDetectionSource {
    pub fn new() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            handle: None,
        }
    }
}

impl DetectionSource for WindowsDetectionSource {
    fn start(&mut self, events: mpsc::Sender<DetectionEvent>) -> Result<()> {
        let stop = Arc::clone(&self.stop);
        self.handle = Some(
            std::thread::Builder::new()
                .name("evertranscript-detect".to_string())
                .spawn(move || {
                    let started = Instant::now();
                    let mut held: BTreeSet<String> = BTreeSet::new();
                    let mut releasing: BTreeMap<String, Instant> = BTreeMap::new();

                    while !stop.load(Ordering::Relaxed) {
                        let now = DetectionInstant(started.elapsed().as_millis() as u64);
                        let current = microphone_holders();

                        for id in current.difference(&held).cloned().collect::<Vec<_>>() {
                            releasing.remove(&id);
                            held.insert(id.clone());
                            let _ = events.blocking_send(DetectionEvent::MicHeld {
                                at: now,
                                app: AppIdentity::bare(&id),
                            });
                        }

                        for id in held.difference(&current).cloned().collect::<Vec<_>>() {
                            let first_seen = *releasing.entry(id.clone()).or_insert(Instant::now());
                            if first_seen.elapsed().as_millis() as u64 >= RELEASE_DEBOUNCE_MS {
                                releasing.remove(&id);
                                held.remove(&id);
                                let _ = events.blocking_send(DetectionEvent::MicReleased {
                                    at: now,
                                    app: AppIdentity::bare(&id),
                                });
                            }
                        }
                        releasing.retain(|id, _| !current.contains(id));

                        if events
                            .blocking_send(DetectionEvent::Tick { at: now })
                            .is_err()
                        {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(POLL_MS));
                    }
                })?,
        );
        Ok(())
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    fn describe(&self) -> String {
        "windows".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The name the detector reports for a process it can certainly see:
    /// this test's own.
    ///
    /// Every Windows row is compared against whatever this function returns,
    /// so a function that answers `None` makes the entire table unreachable
    /// — and `None` is what it answered, for every process on the machine.
    /// `GetModuleBaseNameW` walks the module list and documents a need for
    /// `PROCESS_QUERY_INFORMATION | PROCESS_VM_READ`; the handle is opened
    /// with `PROCESS_QUERY_LIMITED_INFORMATION`, so it failed with
    /// `ERROR_ACCESS_DENIED`, and returning 0 is indistinguishable here from
    /// "no such process".
    ///
    /// The name is taken from `current_exe` rather than written down,
    /// because an expected string this milestone made up rather than read is
    /// how the previous four defects passed their tests.
    #[test]
    fn reports_the_name_of_a_process_it_can_see() {
        let expected = std::env::current_exe()
            .expect("this test has an executable")
            .file_name()
            .expect("which has a file name")
            .to_string_lossy()
            .to_lowercase();

        assert_eq!(executable_name(std::process::id()), Some(expected));
    }

    /// The lowercasing the Watchlist comparison depends on.
    ///
    /// Asserted here rather than against a live process, because a live
    /// process cannot fail it: the test binary's own name is already
    /// lowercase, so deleting the `to_lowercase` would leave every
    /// process-based test still passing. The paths are real ones read off
    /// the machine this ran on; only the casing is varied, which is the
    /// thing under test.
    #[test]
    fn lowercases_the_leaf_of_a_path() {
        assert_eq!(
            leaf_lowercased(r"C:\Program Files (x86)\Microsoft\Edge\Application\MsEdge.EXE")
                .as_deref(),
            Some("msedge.exe")
        );
        assert_eq!(
            leaf_lowercased(
                r"C:\Program Files\WindowsApps\MSTeams_26213.1006.5014.9784_x64__8wekyb3d8bbwe\MS-Teams.exe"
            )
            .as_deref(),
            Some("ms-teams.exe")
        );
        // A bare name, and the empty answer that must not become a match.
        assert_eq!(leaf_lowercased("Zoom.EXE").as_deref(), Some("zoom.exe"));
        assert_eq!(leaf_lowercased("").as_deref(), None);
        assert_eq!(leaf_lowercased(r"C:\dir\").as_deref(), None);
    }

    /// A live process is still named, and still ends up an executable.
    #[test]
    fn reports_a_live_process_as_an_executable() {
        let name = executable_name(std::process::id()).expect("a name for this process");
        assert!(name.ends_with(".exe"), "{name} should be an executable");
    }
}
