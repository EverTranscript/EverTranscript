//! macOS detection: who is holding the microphone.
//!
//! The trigger needs two facts — a Watchlist app, and a live microphone —
//! and CoreAudio's process objects carry both at once. Each object knows the
//! bundle id it belongs to (`kAudioProcessPropertyBundleID`) and whether it
//! is currently recording (`kAudioProcessPropertyIsRunningInput`), so one
//! enumeration answers "which apps are in a call" directly.
//!
//! **NSWorkspace turned out to be unnecessary, and that is a finding rather
//! than a shortcut.** ADR-0024 words the trigger as a Watchlist app being
//! active AND the microphone being in use, and ticket 04 asked for
//! running-application observation to supply the first half. Holding the
//! microphone is strictly stronger evidence than being frontmost: an app
//! recording audio is in a call whether or not its window has focus, which
//! is the same reason the policy latches. Observing app activation as well
//! would add a signal the policy would have to ignore to stay correct.
//!
//! **Polling, not listeners, on purpose.** The absorption catalog describes
//! re-attaching a per-device listener on default-device change, guarding
//! stale callbacks, and polling as a fallback. A poll *is* the fallback with
//! none of the failure modes: there is no listener to go stale when AirPods
//! connect mid-call, which is precisely the churn this has to survive. The
//! cost is one process enumeration every 500 ms, against a 15 s window.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Instant;

use anyhow::Result;
use objc2_core_audio::AudioObjectGetPropertyData;
use objc2_core_audio::AudioObjectGetPropertyDataSize;
use objc2_core_audio::AudioObjectID;
use objc2_core_audio::AudioObjectPropertyAddress;
use objc2_core_audio::kAudioHardwarePropertyProcessObjectList;
use objc2_core_audio::kAudioObjectPropertyElementMain;
use objc2_core_audio::kAudioObjectPropertyScopeGlobal;
use objc2_core_audio::kAudioObjectSystemObject;
use objc2_core_audio::kAudioProcessPropertyBundleID;
use objc2_core_audio::kAudioProcessPropertyIsRunningInput;
use objc2_foundation::NSString;
use std::collections::BTreeSet;
use tokio::sync::mpsc;

use super::AppIdentity;
use super::DetectionEvent;
use super::DetectionInstant;
use super::DetectionSource;
use super::watchlist::responsible_app;
#[cfg(test)]
use crate::audio::AudioSource;

/// How often the machine is asked what it is doing.
///
/// The edge debounce the prior art specifies, expressed as the rate at which
/// an edge can be observed at all: nothing shorter than this is ever
/// reported, so a flap inside one interval is invisible by construction.
pub(crate) const POLL_MS: u64 = 500;

/// How long the microphone must look released before it is believed.
///
/// The catalog's 2 s debounce on the holder list going empty. It belongs
/// here rather than in the policy: the policy's continuity window is a
/// decision about a meeting, this is a decision about whether the *reading*
/// is real, and composing the two would silently make a 15 s window 17 s.
const RELEASE_DEBOUNCE_MS: u64 = 2_000;

fn address(selector: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    }
}

/// Every audio process object the system knows about.
fn process_objects() -> Vec<AudioObjectID> {
    let mut addr = address(kAudioHardwarePropertyProcessObjectList);
    let mut size: u32 = 0;
    let code = unsafe {
        AudioObjectGetPropertyDataSize(
            kAudioObjectSystemObject as AudioObjectID,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
        )
    };
    if code != 0 || size == 0 {
        return Vec::new();
    }
    let mut processes =
        vec![0 as AudioObjectID; size as usize / std::mem::size_of::<AudioObjectID>()];
    let code = unsafe {
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject as AudioObjectID,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(processes.as_mut_slice()).cast::<c_void>(),
        )
    };
    if code != 0 { Vec::new() } else { processes }
}

fn is_running_input(process: AudioObjectID) -> bool {
    let mut addr = address(kAudioProcessPropertyIsRunningInput);
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let code = unsafe {
        AudioObjectGetPropertyData(
            process,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast::<c_void>(),
        )
    };
    code == 0 && value == 1
}

/// The bundle id an audio process belongs to.
///
/// `None` when the platform will not say — a system process, or one that has
/// exited between the enumeration and the question. Treated as "not an app"
/// rather than guessed at: this feeds a decision to record someone.
fn bundle_id(process: AudioObjectID) -> Option<String> {
    let mut addr = address(kAudioProcessPropertyBundleID);
    let mut value: *const NSString = std::ptr::null();
    let mut size = std::mem::size_of::<*const NSString>() as u32;
    let code = unsafe {
        AudioObjectGetPropertyData(
            process,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast::<c_void>(),
        )
    };
    if code != 0 || value.is_null() {
        return None;
    }
    let string = unsafe { objc2::rc::Retained::from_raw(value as *mut NSString) }?;
    let text = string.to_string();
    (!text.is_empty()).then_some(text)
}

/// Which apps are holding the microphone right now, as responsible apps.
pub fn microphone_holders() -> BTreeSet<String> {
    process_objects()
        .into_iter()
        .filter(|process| is_running_input(*process))
        .filter_map(bundle_id)
        // Helpers resolve to the app answerable for them, so policy never
        // sees a renderer (ADR-0030's fragile edge).
        .map(|id| responsible_app(&id))
        .collect()
}

/// How many processes are recording, whether or not they can be named.
///
/// [`microphone_holders`] deliberately drops anything without a bundle id,
/// because a Watchlist entry on macOS *is* a bundle id and an app that has
/// none can never match one. That filter also makes the detector invisible
/// to itself: a bare binary has no bundle id, the same fact that makes
/// macOS label this Core's own status item with its process id. This counts
/// the unfiltered truth, so the mechanism can be verified against a real
/// microphone on a machine where the only thing recording is us.
pub fn recording_process_count() -> usize {
    process_objects()
        .into_iter()
        .filter(|process| is_running_input(*process))
        .count()
}

/// The live macOS DetectionSource.
pub struct MacOsDetectionSource {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Default for MacOsDetectionSource {
    fn default() -> Self {
        Self::new()
    }
}

impl MacOsDetectionSource {
    pub fn new() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            handle: None,
        }
    }
}

impl DetectionSource for MacOsDetectionSource {
    fn start(&mut self, events: mpsc::Sender<DetectionEvent>) -> Result<()> {
        let stop = Arc::clone(&self.stop);
        // A thread rather than a task: the CoreAudio enumeration is
        // blocking, and it must not sit on a runtime worker beside capture.
        self.handle = Some(
            std::thread::Builder::new()
                .name("evertranscript-detect".to_string())
                .spawn(move || {
                    let started = Instant::now();
                    let mut held: BTreeSet<String> = BTreeSet::new();
                    // Releases seen but not yet believed, with when they
                    // were first seen.
                    let mut releasing: std::collections::BTreeMap<String, Instant> =
                        std::collections::BTreeMap::new();

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
                            // Not believed immediately: a device changing
                            // underneath a call drops the holder for a beat,
                            // and reporting that as a release would make the
                            // policy reason about a meeting that never
                            // paused.
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
                        // Anything that came back inside the debounce is no
                        // longer releasing.
                        releasing.retain(|id, _| !current.contains(id));

                        // Deadlines expire on their own; a policy only ever
                        // asked when something changes holds a recording
                        // open forever after the last event.
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
        "macos".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asking_who_holds_the_microphone_answers_rather_than_panicking() {
        // Whatever this machine's answer is, it must be a stated one. The
        // set is usually empty, which is itself the correct answer.
        let holders = microphone_holders();
        for id in &holders {
            assert!(!id.is_empty(), "an empty bundle id is not an app");
            assert!(
                !id.contains(".helper"),
                "{id} reached policy as a helper rather than as its app"
            );
        }
    }

    #[test]
    fn a_real_microphone_hold_is_visible_to_the_detector() {
        // The mechanism, proven against this machine rather than asserted:
        // while a capture stream is open, CoreAudio must report at least one
        // process recording. `microphone_holders` cannot be used for this —
        // a test binary has no bundle id, so it filters itself out, which is
        // correct behaviour and a useless test subject.
        //
        // Deliberately not a before/after delta. The first version of this
        // compared the count at rest against the count during, and it went
        // red as soon as the suite grew: cargo runs tests in parallel, other
        // tests in this crate open capture streams, and "at rest" is a
        // fiction inside a parallel suite. An absolute claim is the one that
        // is actually true.

        let mut source = crate::audio::live::LiveSource::new();
        let (events, _rx) = tokio::sync::mpsc::channel(64);
        if source
            .start(crate::audio::CaptureClock::start(), events)
            .is_err()
        {
            eprintln!("skipping: this machine cannot open a microphone");
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1_200));
        let during = recording_process_count();
        source.stop();

        assert!(
            during >= 1,
            "a capture stream was open and the detector saw no process \
             recording at all — it cannot see a live microphone"
        );
    }

    #[test]
    fn the_enumeration_is_stable_across_calls() {
        // Two reads a moment apart should agree on a quiet machine. A
        // detector that disagrees with itself would flap the policy.
        let first = microphone_holders();
        let second = microphone_holders();
        assert_eq!(first, second, "the same machine reported two ways");
    }
}
