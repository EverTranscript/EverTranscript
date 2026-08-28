//! Meeting Detection: what the machine is doing, and nothing about what is said.
//!
//! This is the M2 twin of [`crate::audio`]'s AudioSource. Capture proved the
//! shape: put one trait between the product and the platform, and everything
//! above it becomes testable without hardware. Detection needs that more, not
//! less — the alternative to a seam here is a policy that can only be
//! exercised by holding a real meeting.
//!
//! Two things are load-bearing:
//!
//! 1. **Events name the responsible app, never the process.** A hot
//!    microphone in `Google Chrome Helper (Renderer)` is Chrome talking, and
//!    the mapping that knows so lives *below* this seam. Policy that had to
//!    understand helper processes would be policy that breaks whenever
//!    Chromium renames one.
//! 2. **Events carry their own time.** The continuity window, the debounces
//!    and the suppression are all durations, and a state machine that reads
//!    the wall clock itself cannot be tested without waiting. Every decision
//!    here is a function of the timeline it was given (ADR-0023 as amended).

pub mod driver;
pub mod fixture;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod notify;
pub mod policy;
pub mod watchlist;
#[cfg(target_os = "windows")]
pub mod windows;

/// Milliseconds since detection started.
///
/// A newtype for the same reason [`crate::audio::CaptureOffset`] is one:
/// this gets compared against durations constantly, and confusing it with a
/// wall-clock epoch is the bug that makes a fifteen-second window fifty
/// years long.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct DetectionInstant(pub u64);

impl DetectionInstant {
    pub const ZERO: Self = Self(0);

    pub fn millis(self) -> u64 {
        self.0
    }

    pub fn plus_millis(self, millis: u64) -> Self {
        Self(self.0 + millis)
    }

    /// How long since `earlier`. Saturating, because a source that stamps
    /// two events out of order must not produce a gigantic duration.
    pub fn since(self, earlier: Self) -> u64 {
        self.0.saturating_sub(earlier.0)
    }
}

/// An application, as the Operator would name it.
///
/// Always the responsible app: helper processes are mapped before an event
/// reaches this type, so `Google Chrome Helper` never appears.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AppIdentity {
    /// Bundle id on macOS, executable name on Windows. The stable key.
    pub id: String,
    /// What to show a human. Falls back to `id` when the platform has
    /// nothing better.
    pub name: String,
}

impl AppIdentity {
    pub fn new(id: &str, name: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
        }
    }

    /// An app known only by its identifier — the honest state when the
    /// platform will tell us what is running but not what it is called.
    pub fn bare(id: &str) -> Self {
        Self::new(id, id)
    }
}

/// A calendar event, as much of one as arming needs (ADR-0036).
///
/// The title is content, and the only content this module ever sees. It
/// arrives solely under a grant the Operator may skip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarEvent {
    pub id: String,
    pub title: String,
    pub attendees: Vec<String>,
    /// When the event is scheduled to end, on the detection clock. Feeds the
    /// auto-stop window rather than stopping anything itself.
    pub scheduled_end: Option<DetectionInstant>,
}

/// Something the machine did.
///
/// Deliberately small. Every variant is state a platform can observe without
/// a permission the product does not already hold — with the calendar as the
/// stated exception (ADR-0036).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetectionEvent {
    /// This app is running and frontmost-or-active. Not "a window appeared".
    AppActive {
        at: DetectionInstant,
        app: AppIdentity,
    },
    /// This app is no longer active or no longer running.
    AppGone {
        at: DetectionInstant,
        app: AppIdentity,
    },
    /// This app now holds the microphone.
    MicHeld {
        at: DetectionInstant,
        app: AppIdentity,
    },
    /// This app released the microphone.
    MicReleased {
        at: DetectionInstant,
        app: AppIdentity,
    },
    /// A scheduled meeting has reached its start time (ADR-0036). Arms
    /// detection and names the Meeting; never starts a recording.
    CalendarEventStarted {
        at: DetectionInstant,
        event: CalendarEvent,
    },
    /// A scheduled meeting has reached its end time.
    CalendarEventEnded { at: DetectionInstant, id: String },
    /// Time passed and nothing happened.
    ///
    /// Not noise: deadlines expire on their own, and a policy that is only
    /// ever asked a question when the world changes will hold a recording
    /// open forever after the last event. A live source emits these on a
    /// timer; the fixture emits them where the script says.
    Tick { at: DetectionInstant },
}

impl DetectionEvent {
    pub fn at(&self) -> DetectionInstant {
        match self {
            Self::AppActive { at, .. }
            | Self::AppGone { at, .. }
            | Self::MicHeld { at, .. }
            | Self::MicReleased { at, .. }
            | Self::CalendarEventStarted { at, .. }
            | Self::CalendarEventEnded { at, .. }
            | Self::Tick { at } => *at,
        }
    }

    /// The app this event is about, when it is about one.
    pub fn app(&self) -> Option<&AppIdentity> {
        match self {
            Self::AppActive { app, .. }
            | Self::AppGone { app, .. }
            | Self::MicHeld { app, .. }
            | Self::MicReleased { app, .. } => Some(app),
            _ => None,
        }
    }
}

/// The seam every policy test drives.
///
/// Live detection and a scripted timeline are interchangeable at exactly
/// this point, which is why "does Auto-Record behave" is answerable without
/// a meeting, a browser, or a second machine.
pub trait DetectionSource: Send {
    /// Begins producing events. Called once per source instance.
    fn start(&mut self, events: tokio::sync::mpsc::Sender<DetectionEvent>) -> anyhow::Result<()>;

    /// Stops producing. Must be safe to call more than once.
    fn stop(&mut self);

    /// For logs and errors.
    fn describe(&self) -> String;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_instant_never_produces_a_negative_duration() {
        // Two sources disagreeing by a millisecond must not turn a 15 s
        // window into a 584-million-year one.
        let early = DetectionInstant(100);
        let late = DetectionInstant(50);
        assert_eq!(late.since(early), 0);
        assert_eq!(early.since(late), 50);
    }

    #[test]
    fn every_event_carries_its_time() {
        // The policy is a function of the timeline. An event that cannot say
        // when it happened is one the policy cannot reason about.
        let at = DetectionInstant(1_234);
        let app = AppIdentity::bare("us.zoom.xos");
        for event in [
            DetectionEvent::AppActive {
                at,
                app: app.clone(),
            },
            DetectionEvent::AppGone {
                at,
                app: app.clone(),
            },
            DetectionEvent::MicHeld {
                at,
                app: app.clone(),
            },
            DetectionEvent::MicReleased { at, app },
            DetectionEvent::CalendarEventEnded { at, id: "e".into() },
            DetectionEvent::Tick { at },
        ] {
            assert_eq!(event.at(), at, "{event:?} lost its timestamp");
        }
    }
}
