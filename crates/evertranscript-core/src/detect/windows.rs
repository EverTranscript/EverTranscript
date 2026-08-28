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
//! **Verification status, stated plainly.** The API usage below is
//! typechecked against `x86_64-pc-windows-msvc`. It has never been run: this
//! was written on an Apple Silicon Mac, and ticket 05 is not complete until
//! it has been exercised on a real Windows 10+ machine with the per-browser
//! matrix from ticket 09. Anything else would be a checked box nobody looked
//! at, which is the exact failure this milestone's ticket 10 postmortem
//! exists to prevent.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Instant;

use anyhow::Result;
use tokio::sync::mpsc;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Media::Audio::AudioSessionStateActive;
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
use windows::Win32::System::ProcessStatus::GetModuleBaseNameW;
use windows::Win32::System::Threading::OpenProcess;
use windows::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION;
use windows::core::Interface;

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
fn executable_name(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 260];
        let written = GetModuleBaseNameW(handle, None, &mut buffer);
        let _ = CloseHandle(handle);
        if written == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buffer[..written as usize]).to_lowercase())
    }
}

/// Which apps are holding the microphone right now, as responsible apps.
pub fn microphone_holders() -> BTreeSet<String> {
    let mut holders = BTreeSet::new();
    unsafe {
        // Already-initialised is not an error here: the Core may have a COM
        // apartment from capture already.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let Ok(enumerator) =
            CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
        else {
            return holders;
        };
        // The capture endpoint: a machine with no microphone answers
        // "nobody", which is the truthful answer rather than an error.
        let Ok(device): windows::core::Result<IMMDevice> =
            enumerator.GetDefaultAudioEndpoint(eCapture, eMultimedia)
        else {
            return holders;
        };
        let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
            return holders;
        };
        let Ok(sessions) = manager.GetSessionEnumerator() else {
            return holders;
        };
        let Ok(count) = sessions.GetCount() else {
            return holders;
        };

        for index in 0..count {
            let Ok(control) = sessions.GetSession(index) else {
                continue;
            };
            let Ok(control2) = control.cast::<IAudioSessionControl2>() else {
                continue;
            };
            // Inactive and expired sessions linger; only an active one is a
            // live microphone.
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
