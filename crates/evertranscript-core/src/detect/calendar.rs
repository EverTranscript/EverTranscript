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
//! the Meeting. The event carries its scheduled end as well, which lengthens
//! the continuity window when a browser meeting goes quiet early (the policy
//! says by how much, and why no more). Capture still begins
//! only on the Watchlist-and-microphone trigger: the calendar knows *when*,
//! only the microphone knows *that*.
//!
//! An event title is content, and this is the one place the product reads
//! any — under a grant the Operator may decline, which ADR-0036 made the
//! honest wording of Nothing Ambient.

use std::collections::BTreeMap;
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

/// How often a sleeping poll looks for the stop flag.
const STOP_CHECK_MS: u64 = 250;

/// How far back a reading reaches. Longer than any meeting, so one that
/// began before the Core did still arms, and a store that matches a range
/// by start time rather than by overlap cannot end a meeting early.
const LOOKBACK_SECS: f64 = 12.0 * 60.0 * 60.0;

/// An event as one reading of the store found it, before anything decides
/// what it means. Times are seconds from the moment of reading.
struct Reading {
    id: String,
    title: String,
    attendees: Vec<String>,
    starts_in: f64,
    ends_in: f64,
    all_day: bool,
}

/// What changed since the last reading: a meeting in progress is announced
/// once, and one that is over, or gone from the store, is announced as
/// ended.
///
/// A reading holds more than the meetings in progress — meetings already
/// over, and whatever a store's range returns — so this is the one place
/// that decides a meeting has started. `announced` keeps each one's latest
/// scheduled end, for [`overdue`].
fn changes(
    announced: &mut BTreeMap<String, DetectionInstant>,
    readings: Vec<Reading>,
    now: DetectionInstant,
) -> Vec<DetectionEvent> {
    let mut changed = Vec::new();
    let mut live = BTreeSet::new();
    for reading in readings {
        // A day, not a meeting: a holiday or an out-of-office would
        // otherwise arm at midnight and name whatever is recorded that day.
        // anarlog skips them on every path that acts on an event.
        if reading.all_day || reading.starts_in > 0.0 || reading.ends_in <= 0.0 {
            continue;
        }
        live.insert(reading.id.clone());
        // On the detection clock rather than as a wall time: it is the clock
        // the continuity window counts on.
        let end = now.plus_millis((reading.ends_in * 1000.0) as u64);
        if announced.insert(reading.id.clone(), end).is_none() {
            debug!(event = reading.id, "a scheduled meeting has started");
            changed.push(DetectionEvent::CalendarEventStarted {
                at: now,
                event: CalendarEvent {
                    id: reading.id,
                    // The store's own fallback, so an untitled event still
                    // names its Meeting something.
                    title: if reading.title.trim().is_empty() {
                        "Untitled event".to_string()
                    } else {
                        reading.title
                    },
                    attendees: reading.attendees,
                    scheduled_end: Some(end),
                },
            });
        }
    }
    announced.retain(|id, _| {
        let over = !live.contains(id);
        if over {
            changed.push(DetectionEvent::CalendarEventEnded {
                at: now,
                id: id.clone(),
            });
        }
        !over
    });
    changed
}

/// What a store that could not be read still settles: a meeting is over
/// once its last known scheduled end has passed.
///
/// Without it, a store that stays unreadable never ends a meeting, and the
/// armed meeting names whatever Auto-Record starts next, hours later.
fn overdue(
    announced: &mut BTreeMap<String, DetectionInstant>,
    now: DetectionInstant,
) -> Vec<DetectionEvent> {
    let mut changed = Vec::new();
    announced.retain(|id, end| {
        let over = *end <= now;
        if over {
            changed.push(DetectionEvent::CalendarEventEnded {
                at: now,
                id: id.clone(),
            });
        }
        !over
    });
    changed
}

/// Wall time as Windows keeps it, which is what a WinRT `DateTime` holds:
/// 100 ns ticks since 1601.
#[cfg(any(target_os = "windows", test))]
fn winrt_ticks(time: std::time::SystemTime) -> i64 {
    const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;
    let since = time
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    UNIX_EPOCH_TICKS + (since.as_nanos() / 100) as i64
}

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
    use std::panic::AssertUnwindSafe;

    use super::*;
    use objc2::rc::Retained;
    use objc2_event_kit::EKAuthorizationStatus;
    use objc2_event_kit::EKEntityType;
    use objc2_event_kit::EKEventStore;
    use objc2_foundation::NSDate;
    use tracing::warn;

    thread_local! {
        /// The polling thread's one store. Apple's header says it is
        /// "generally best to hold onto a long-lived instance of an event
        /// store", and anarlog keeps a single shared one
        /// (`crates/apple-calendar/src/apple/handle.rs`). This one belongs to
        /// the polling thread, the only reader, so it never crosses threads.
        static STORE: Retained<EKEventStore> = unsafe { EKEventStore::new() };
    }

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

    /// Asks macOS for full access — the one call that shows the Calendars
    /// prompt and lists the app under Privacy & Security, where the Operator
    /// can change their answer later. Blocks until they answer. An app
    /// already refused, or one that cannot prompt (no calendars entitlement
    /// under the hardened runtime), gets `Withheld` at once with no dialog;
    /// so does one nobody answers within the wait, and a later poll picks
    /// up whatever they eventually chose.
    ///
    /// The prompt names the process's *responsible* app: the Client when it
    /// spawned this Core. The grant is keyed on the app bundle, so it also
    /// covers this Core when the login item starts it; a copy of the binary
    /// outside the bundle is a stranger to it (probed 2026-09-15).
    pub fn request() -> Access {
        use std::sync::mpsc;
        use std::time::Duration;

        use block2::RcBlock;
        use objc2::runtime::Bool;
        use objc2_foundation::NSError;

        /// Long enough to read the dialog; short enough that a Client
        /// waiting on the answer is not waiting forever.
        const ANSWER_WAIT: Duration = Duration::from_secs(300);

        if access() == Access::Granted {
            return Access::Granted;
        }
        let (tx, rx) = mpsc::channel::<bool>();
        let completion = RcBlock::new(move |granted: Bool, _error: *mut NSError| {
            let _ = tx.send(granted.as_bool());
        });
        // Its own store: this runs on whichever thread carried the request,
        // not the polling thread, and the grant is process-wide anyway.
        let store = unsafe { EKEventStore::new() };
        let asked = objc2::exception::catch(AssertUnwindSafe(|| unsafe {
            store.requestFullAccessToEventsWithCompletion(RcBlock::as_ptr(&completion));
        }));
        if let Err(exception) = asked {
            warn!(?exception, "the calendar access request could not be made");
            return Access::Withheld;
        }
        // The completion arrives on a system queue, never on this thread,
        // so waiting here cannot deadlock.
        match rx.recv_timeout(ANSWER_WAIT) {
            Ok(true) => Access::Granted,
            Ok(false) => Access::Withheld,
            Err(_) => access(),
        }
    }

    /// Events that began within the lookback, or `None` when the store
    /// could not be read this time.
    pub fn read() -> Option<Vec<Reading>> {
        if access() != Access::Granted {
            return Some(Vec::new());
        }
        STORE.with(|store| unsafe {
            // These two calls can raise an Objective-C exception, and one
            // that unwinds into Rust aborts the Core. anarlog catches the
            // same two and takes the exception for a failed XPC connection
            // to the calendar daemon, which it retries; here the next poll
            // is the retry.
            let store = AssertUnwindSafe(store);
            let fetched = objc2::exception::catch(|| {
                let from = NSDate::dateWithTimeIntervalSinceNow(-LOOKBACK_SECS);
                let until = NSDate::date();
                let predicate =
                    store.predicateForEventsWithStartDate_endDate_calendars(&from, &until, None);
                store.eventsMatchingPredicate(&predicate)
            });
            let events = match fetched {
                Ok(events) => events,
                Err(exception) => {
                    warn!(?exception, "the calendar store could not be read");
                    return None;
                }
            };

            let readings = events
                .iter()
                .filter_map(|event| {
                    Some(Reading {
                        id: event.eventIdentifier()?.to_string(),
                        title: event.title().to_string(),
                        attendees: event
                            .attendees()
                            .map(|list| {
                                list.iter()
                                    .filter_map(|attendee| {
                                        attendee.name().map(|name| name.to_string())
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        starts_in: event.startDate().timeIntervalSinceNow(),
                        ends_in: event.endDate().timeIntervalSinceNow(),
                        all_day: event.isAllDay(),
                    })
                })
                .collect();
            Some(readings)
        })
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
/// **Verification status.** Run once, on Windows 11 Pro 26200 on
/// 2026-09-15, with the Core given package identity by a signed sparse
/// package declaring the `appointments` capability. That was a test
/// package; nothing ships one yet (`DECISIONS.md` Q100, Q103). Against a
/// calendar of test appointments, one already in progress armed on the
/// first poll, and one that began about five minutes into the run armed on
/// the first poll after its start. One later that day and an all-day one did not arm. The
/// Core logged each title, and a test binary asking for the same properties
/// read the invitees. The store matches a range by overlap and keeps start
/// times to the whole minute.
///
/// Never observed: a Meeting named by an appointment, which needs a real
/// capture, and an appointment anyone actually scheduled. That machine's
/// store held none until the test added some.
#[cfg(target_os = "windows")]
mod eventkit {
    use super::*;
    use windows::ApplicationModel::Appointments::AppointmentManager;
    use windows::ApplicationModel::Appointments::AppointmentProperties;
    use windows::ApplicationModel::Appointments::AppointmentStore;
    use windows::ApplicationModel::Appointments::AppointmentStoreAccessType;
    use windows::ApplicationModel::Appointments::FindAppointmentsOptions;
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

    /// Asking for the store *is* the request on Windows: the capability is
    /// the package's, and the first `RequestStoreAsync` is what the system
    /// prompts on, when it prompts at all.
    pub fn request() -> Access {
        access()
    }

    /// Appointments that began within the lookback, or `None` when the
    /// store could not be read this time.
    pub fn read() -> Option<Vec<Reading>> {
        // WinRT counts in 100 ns ticks.
        const TICKS_PER_SECOND: i64 = 10_000_000;

        let Some(store) = store() else {
            return Some(Vec::new());
        };
        let Ok(options) = FindAppointmentsOptions::new() else {
            return None;
        };
        // A query loads almost nothing it is not asked for
        // (`FindAppointmentsAsync`'s remarks), and an appointment read
        // without these has no time to arm at and no title to name.
        if let Ok(fetch) = options.FetchProperties() {
            for name in [
                AppointmentProperties::Subject(),
                AppointmentProperties::StartTime(),
                AppointmentProperties::Duration(),
                AppointmentProperties::AllDay(),
                AppointmentProperties::Invitees(),
            ]
            .into_iter()
            .flatten()
            {
                let _ = fetch.Append(&name);
            }
        }
        let now = winrt_ticks(std::time::SystemTime::now());
        let lookback = (LOOKBACK_SECS as i64) * TICKS_PER_SECOND;
        let Ok(found) = store
            .FindAppointmentsAsyncWithOptions(
                DateTime {
                    UniversalTime: now - lookback,
                },
                TimeSpan { Duration: lookback },
                &options,
            )
            .and_then(block_on)
        else {
            return None;
        };
        let seconds = |ticks: i64| ticks as f64 / TICKS_PER_SECOND as f64;
        let readings = found
            .into_iter()
            .filter_map(|appointment| {
                let start = appointment.StartTime().ok()?.UniversalTime;
                let end = start + appointment.Duration().ok()?.Duration;
                Some(Reading {
                    id: appointment.LocalId().ok()?.to_string(),
                    title: appointment
                        .Subject()
                        .map(|subject| subject.to_string())
                        .unwrap_or_default(),
                    attendees: appointment
                        .Invitees()
                        .map(|invitees| {
                            invitees
                                .into_iter()
                                .filter_map(|invitee| invitee.DisplayName().ok())
                                .map(|name| name.to_string())
                                .filter(|name| !name.trim().is_empty())
                                .collect()
                        })
                        .unwrap_or_default(),
                    starts_in: seconds(start - now),
                    ends_in: seconds(end - now),
                    all_day: appointment.AllDay().unwrap_or(false),
                })
            })
            .collect();
        Some(readings)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod eventkit {
    use super::*;

    pub fn access() -> Access {
        Access::Withheld
    }

    pub fn request() -> Access {
        Access::Withheld
    }

    pub fn read() -> Option<Vec<Reading>> {
        Some(Vec::new())
    }
}

pub use eventkit::access;
pub use eventkit::request;

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
        // Polling begins whether or not access has been granted: the grant
        // can arrive while the Core runs — from onboarding, from the trust
        // surface, from System Settings — and each poll asks again, so it
        // takes effect within a poll rather than at the next launch. Until
        // then a read is empty and costs one status check.
        if access() != Access::Granted {
            info!("no calendar access; meetings will not be armed or named in advance");
        }
        let stop = Arc::clone(&self.stop);
        self.handle = Some(
            std::thread::Builder::new()
                .name("evertranscript-calendar".to_string())
                .spawn(move || {
                    let started = Instant::now();
                    let mut announced = BTreeMap::new();

                    while !stop.load(Ordering::Relaxed) {
                        let now = DetectionInstant(started.elapsed().as_millis() as u64);
                        let changed = match eventkit::read() {
                            Some(readings) => changes(&mut announced, readings, now),
                            // A store that could not be read says nothing
                            // about which meetings are on. Reading it as
                            // empty would end every one and arm them all
                            // again at the next poll, so they stay armed
                            // until it can be read or they are scheduled to
                            // end.
                            None => overdue(&mut announced, now),
                        };
                        for change in changed {
                            let _ = events.blocking_send(change);
                        }
                        // In slices, so a stop — the Core shutting down — is
                        // honoured within a moment rather than at the next
                        // poll.
                        let mut waited = 0;
                        while waited < POLL_MS && !stop.load(Ordering::Relaxed) {
                            std::thread::sleep(std::time::Duration::from_millis(STOP_CHECK_MS));
                            waited += STOP_CHECK_MS;
                        }
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
        // It polls anyway, so a grant given later is seen; stopping must not
        // wait out the poll interval.
        let stopping = Instant::now();
        source.stop();
        assert!(
            stopping.elapsed() < std::time::Duration::from_millis(POLL_MS),
            "stop waited for the whole poll interval"
        );
        assert!(rx.try_recv().is_err(), "nothing should have been emitted");
    }

    fn reading(id: &str, starts_in: f64, ends_in: f64) -> Reading {
        Reading {
            id: id.to_string(),
            title: "Standup".to_string(),
            attendees: Vec::new(),
            starts_in,
            ends_in,
            all_day: false,
        }
    }

    #[test]
    fn a_meeting_arms_when_it_starts_not_when_it_is_first_seen() {
        let mut announced = BTreeMap::new();
        let at = DetectionInstant;

        // Fifty-five minutes out. Arming now would name the next hour's
        // recordings after it and follow up two minutes later.
        assert!(changes(&mut announced, vec![reading("e", 3300.0, 5100.0)], at(0)).is_empty());

        let started = changes(&mut announced, vec![reading("e", -10.0, 1790.0)], at(1000));
        assert!(
            matches!(
                &started[..],
                [DetectionEvent::CalendarEventStarted { event, .. }]
                    if event.id == "e" && event.scheduled_end == Some(at(1_791_000))
            ),
            "{started:?}"
        );
        assert!(
            changes(
                &mut announced,
                vec![reading("e", -40.0, 1760.0)],
                at(31_000)
            )
            .is_empty(),
            "announced once, however many readings still hold it"
        );

        // Over, whether the store still lists it or has dropped it.
        let mut dropped = announced.clone();
        let over = changes(
            &mut announced,
            vec![reading("e", -1800.0, -1.0)],
            at(1_800_000),
        );
        let gone = changes(&mut dropped, Vec::new(), at(1_800_000));
        for ended in [over, gone] {
            assert!(
                matches!(&ended[..], [DetectionEvent::CalendarEventEnded { id, .. }] if id == "e"),
                "{ended:?}"
            );
        }
    }

    #[test]
    fn an_all_day_entry_is_not_a_meeting() {
        let holiday = Reading {
            all_day: true,
            ..reading("holiday", -3600.0, 72_000.0)
        };
        assert!(changes(&mut BTreeMap::new(), vec![holiday], DetectionInstant(0)).is_empty());
    }

    #[test]
    fn a_store_that_stops_answering_still_ends_a_meeting_on_schedule() {
        let mut announced = BTreeMap::new();
        let at = DetectionInstant;
        changes(&mut announced, vec![reading("e", -10.0, 1790.0)], at(0));
        // Moved half an hour later while the store could still say so.
        changes(
            &mut announced,
            vec![reading("e", -40.0, 3560.0)],
            at(30_000),
        );

        assert!(
            overdue(&mut announced, at(1_800_000)).is_empty(),
            "still on by its latest end, not the one it armed with"
        );
        let ended = overdue(&mut announced, at(3_590_000));
        assert!(
            matches!(&ended[..], [DetectionEvent::CalendarEventEnded { id, .. }] if id == "e"),
            "{ended:?}"
        );
        assert!(announced.is_empty(), "ended once");
    }

    #[test]
    fn windows_time_counts_from_1601() {
        // 2000-01-01T00:00:00Z, worked out from the calendar rather than
        // from the constant under test.
        let y2k = std::time::UNIX_EPOCH + std::time::Duration::from_secs(946_684_800);
        assert_eq!(winrt_ticks(y2k), 125_911_584_000_000_000);
    }
}
