//! The calendar: it arms and names, it never triggers (ADR-0036).
//!
//! Read from the OS store — EventKit here, the WinRT appointment store on
//! Windows — and **never a cloud calendar API**: no OAuth, no token
//! lifecycle, no new entry in Sanctioned Traffic. The calendars an Operator
//! syncs through Internet Accounts are already in the local store, which is
//! the whole reason this is possible without a network.
//!
//! What it may do is bounded on purpose. At a scheduled start it emits
//! [`DetectionEvent::CalendarEventStarted`], which arms detection and names
//! the Meeting; the scheduled end feeds the auto-stop window. Capture still
//! begins only on the Watchlist-and-microphone trigger: the calendar knows
//! *when*, only the microphone knows *that*.
//!
//! An event title is content, and this is the one place the product reads
//! any — under a grant the Operator may decline, which ADR-0036 made the
//! honest wording of Nothing Ambient.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Instant;

use anyhow::Result;
use tokio::sync::mpsc;
use tracing::debug;
use tracing::info;

use super::CalendarEvent;
use super::DetectionEvent;
use super::DetectionInstant;
use super::DetectionSource;

/// How often the calendar is consulted. Minutes matter here, not
/// milliseconds: this arms a meeting, it does not decide anything.
const POLL_MS: u64 = 30_000;

/// How far ahead to look.
const HORIZON_SECS: f64 = 60.0 * 60.0;

/// Whether this machine will let us read the calendar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Granted; events will arrive.
    Granted,
    /// Declined, or never asked. The product works without it — an Operator
    /// who skips this gets everything except the niceties (ADR-0036).
    Withheld,
}

#[cfg(target_os = "macos")]
mod eventkit {
    use super::*;
    use objc2_event_kit::EKAuthorizationStatus;
    use objc2_event_kit::EKEntityType;
    use objc2_event_kit::EKEventStore;
    use objc2_foundation::NSDate;

    pub fn access() -> Access {
        // Asked, never assumed: the status is readable without prompting,
        // and prompting is the onboarding step's job rather than a
        // background poll's.
        let status = unsafe { EKEventStore::authorizationStatusForEntityType(EKEntityType::Event) };
        // `FullAccess` only: the deprecated `Authorized` maps onto it, and
        // write-only access cannot read an event's title, which is the
        // whole point of asking.
        match status {
            EKAuthorizationStatus::FullAccess => Access::Granted,
            _ => Access::Withheld,
        }
    }

    /// Events starting within the horizon, as the policy understands them.
    pub fn upcoming(now: DetectionInstant) -> Vec<CalendarEvent> {
        if access() != Access::Granted {
            return Vec::new();
        }
        unsafe {
            let store = EKEventStore::new();
            let from = NSDate::date();
            let until = NSDate::dateWithTimeIntervalSinceNow(HORIZON_SECS);
            let predicate =
                store.predicateForEventsWithStartDate_endDate_calendars(&from, &until, None);
            let events = store.eventsMatchingPredicate(&predicate);

            events
                .iter()
                .filter_map(|event| {
                    let id = event.eventIdentifier()?.to_string();
                    let title = {
                        let title = event.title().to_string();
                        // The store's own fallback, so an untitled event
                        // still names its Meeting something.
                        if title.trim().is_empty() {
                            "Untitled event".to_string()
                        } else {
                            title
                        }
                    };
                    let attendees = event
                        .attendees()
                        .map(|list| {
                            list.iter()
                                .filter_map(|attendee| attendee.name().map(|name| name.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    // The end, expressed on the detection clock rather than
                    // as a wall time, because that is what the window reads.
                    let scheduled_end = {
                        let seconds = event.endDate().timeIntervalSinceNow().max(0.0);
                        Some(now.plus_millis((seconds * 1000.0) as u64))
                    };
                    Some(CalendarEvent {
                        id,
                        title,
                        attendees,
                        scheduled_end,
                    })
                })
                .collect()
        }
    }
}

/// The WinRT appointment store (ADR-0025 as amended: this milestone, not
/// after it).
///
/// Same bounds as EventKit — local store, never a cloud API, read-only —
/// through a different shape: WinRT asks asynchronously, and the store is
/// consulted from a polling thread every thirty seconds, so a bounded spin
/// is honest here rather than a runtime to yield to.
///
/// **Typechecked against `x86_64-pc-windows-msvc`, never executed.** Same
/// status as the Windows detector, and for the same reason.
#[cfg(target_os = "windows")]
mod eventkit {
    use super::*;
    use windows::ApplicationModel::Appointments::AppointmentManager;
    use windows::ApplicationModel::Appointments::AppointmentStore;
    use windows::ApplicationModel::Appointments::AppointmentStoreAccessType;
    use windows::Foundation::DateTime;
    use windows::Foundation::TimeSpan;
    use windows::Win32::Foundation::APPMODEL_ERROR_NO_PACKAGE;
    use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;
    use windows::Win32::System::Com::COINIT_MULTITHREADED;
    use windows::Win32::System::Com::CoInitializeEx;
    use windows_future::AsyncStatus;
    use windows_future::IAsyncOperation;

    /// Blocks on a WinRT async operation, with a bound.
    ///
    /// Bounded because this runs on a polling thread with nothing waiting on
    /// it, and an operation that never completes would otherwise spin for
    /// the life of the Core — a calendar that hangs must degrade to a
    /// calendar that is unavailable, which the product already handles.
    fn block_on<T: windows::core::RuntimeType + 'static>(
        operation: IAsyncOperation<T>,
    ) -> windows::core::Result<T> {
        for _ in 0..250 {
            match operation.Status()? {
                AsyncStatus::Started => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                _ => return operation.GetResults(),
            }
        }
        let _ = operation.Cancel();
        Err(windows::core::Error::from(
            windows::Win32::Foundation::E_ABORT,
        ))
    }

    /// Whether this process has a package identity.
    ///
    /// `AppointmentManager` is one of the WinRT APIs that requires one, and
    /// an unpackaged process does not merely get an error from it — the
    /// Windows CI runner exited abnormally rather than failing a test.
    /// Asking first turns a crash into the honest answer, which is that the
    /// appointment store is unavailable to a binary run from a folder.
    fn is_packaged() -> bool {
        let mut length: u32 = 0;
        let code = unsafe { GetCurrentPackageFullName(&mut length, None) };
        code != APPMODEL_ERROR_NO_PACKAGE
    }

    fn store() -> Option<AppointmentStore> {
        if !is_packaged() {
            return None;
        }
        // WinRT needs an apartment on this thread before anything else is
        // called. The detector does this and the calendar did not, which is
        // undefined rather than merely unsupported: on a CI runner the test
        // binary did not fail an assertion, it exited abnormally.
        // Already-initialised is not an error — the Core may have an
        // apartment from capture already.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        AppointmentManager::RequestStoreAsync(AppointmentStoreAccessType::AllCalendarsReadOnly)
            .and_then(block_on)
            .ok()
    }

    pub fn access() -> Access {
        // Read-only, all calendars: the narrowest access that can answer
        // "what is scheduled now", and it cannot write to anyone's calendar.
        match store() {
            Some(_) => Access::Granted,
            None => Access::Withheld,
        }
    }

    pub fn upcoming(now: DetectionInstant) -> Vec<CalendarEvent> {
        let Some(store) = store() else {
            return Vec::new();
        };
        let from = DateTime { UniversalTime: 0 };
        let horizon = TimeSpan {
            // WinRT counts in 100 ns ticks.
            Duration: (HORIZON_SECS as i64) * 10_000_000,
        };
        let Ok(found) = store
            .FindAppointmentsAsync(from, horizon)
            .and_then(block_on)
        else {
            return Vec::new();
        };
        found
            .into_iter()
            .map(|appointment| {
                let title = appointment
                    .Subject()
                    .map(|subject| subject.to_string())
                    .unwrap_or_default();
                let seconds = appointment
                    .Duration()
                    .map(|duration| duration.Duration as f64 / 10_000_000.0)
                    .unwrap_or(0.0);
                CalendarEvent {
                    id: appointment
                        .LocalId()
                        .map(|id| id.to_string())
                        .unwrap_or_default(),
                    title: if title.trim().is_empty() {
                        "Untitled event".to_string()
                    } else {
                        title
                    },
                    attendees: Vec::new(),
                    scheduled_end: Some(now.plus_millis((seconds.max(0.0) * 1000.0) as u64)),
                }
            })
            .collect()
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod eventkit {
    use super::*;

    pub fn access() -> Access {
        Access::Withheld
    }

    pub fn upcoming(_now: DetectionInstant) -> Vec<CalendarEvent> {
        Vec::new()
    }
}

pub use eventkit::access;

/// Emits calendar events as they start and end.
pub struct CalendarSource {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Default for CalendarSource {
    fn default() -> Self {
        Self::new()
    }
}

impl CalendarSource {
    pub fn new() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            handle: None,
        }
    }
}

impl DetectionSource for CalendarSource {
    fn start(&mut self, events: mpsc::Sender<DetectionEvent>) -> Result<()> {
        if access() != Access::Granted {
            info!("no calendar access; meetings will not be armed or named in advance");
            return Ok(());
        }
        let stop = Arc::clone(&self.stop);
        self.handle = Some(
            std::thread::Builder::new()
                .name("evertranscript-calendar".to_string())
                .spawn(move || {
                    let started = Instant::now();
                    let mut announced: BTreeSet<String> = BTreeSet::new();

                    while !stop.load(Ordering::Relaxed) {
                        let now = DetectionInstant(started.elapsed().as_millis() as u64);
                        let upcoming = eventkit::upcoming(now);
                        let live: BTreeSet<String> =
                            upcoming.iter().map(|event| event.id.clone()).collect();

                        for event in upcoming {
                            if announced.insert(event.id.clone()) {
                                debug!(event = event.id, "a scheduled meeting has started");
                                let _ =
                                    events.blocking_send(DetectionEvent::CalendarEventStarted {
                                        at: now,
                                        event,
                                    });
                            }
                        }

                        // Gone from the window means over.
                        for id in announced.difference(&live).cloned().collect::<Vec<_>>() {
                            announced.remove(&id);
                            let _ = events
                                .blocking_send(DetectionEvent::CalendarEventEnded { at: now, id });
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
        "calendar".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_is_answered_rather_than_assumed() {
        // Whatever this machine's answer is, asking must not prompt, hang or
        // panic — a background poll that opens a permission dialog is a
        // product that asks at the worst possible moment.
        let answer = access();
        assert!(matches!(answer, Access::Granted | Access::Withheld));
    }

    #[test]
    fn a_withheld_calendar_produces_no_events_rather_than_an_error() {
        // ADR-0036: skipping the grant costs the niceties and nothing else.
        if access() == Access::Granted {
            return;
        }
        let mut source = CalendarSource::new();
        let (tx, mut rx) = mpsc::channel(8);
        source
            .start(tx)
            .expect("starting without access is not an error");
        source.stop();
        assert!(rx.try_recv().is_err(), "nothing should have been emitted");
    }
}
