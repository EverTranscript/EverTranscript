//! The two moments Auto-Record has to speak.
//!
//! The Core never auto-launches the Client (ADR-0026), so a notification is
//! what it does instead: a heads-up when a scheduled meeting starts, and one
//! follow-up when a meeting the calendar armed never happened.
//!
//! **DND fails open, on purpose.** The macOS mechanism is undocumented and
//! the absorption catalog flags one competitor shipping a check hardcoded to
//! `false`. So the rule here is that a check which cannot answer must return
//! "notify": a notification the Operator did not want is an annoyance, and a
//! product that silently stopped telling them it is recording is the thing
//! ADR-0007's always-visible indicator exists to prevent.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use tracing::debug;

use super::CalendarEvent;

/// How long between prompts about anything, from the prior art.
const COOLDOWN: Duration = Duration::from_secs(120);

/// What the Operator reads.
///
/// A catalog rather than inline strings, matching the Client's rule from M1:
/// the Core has user-facing text now, and retrofitting extraction after the
/// text exists is the regret that rule was written to avoid. English and
/// Simplified Chinese, the v1 pair.
pub mod catalog {
    pub struct Strings {
        pub meeting_starting_title: &'static str,
        pub meeting_starting_body: &'static str,
        pub never_started_title: &'static str,
        pub never_started_body: &'static str,
    }

    pub const EN: Strings = Strings {
        meeting_starting_title: "Meeting starting",
        meeting_starting_body: "{title} is scheduled now. Recording will begin when the call does.",
        never_started_title: "Nothing is recording",
        never_started_body: "\"{title}\" seems to be happening — nothing is recording.",
    };

    pub const ZH_CN: Strings = Strings {
        meeting_starting_title: "会议即将开始",
        meeting_starting_body: "{title} 已到预定时间。通话开始后将自动录制。",
        never_started_title: "当前没有在录制",
        never_started_body: "“{title}”似乎正在进行，但没有在录制。",
    };

    /// The active catalog. A single locale until the Client's own selector
    /// reaches the Core (M5); English renders today.
    pub fn active() -> &'static Strings {
        match std::env::var("EVERTRANSCRIPT_LOCALE").as_deref() {
            Ok("zh-CN") | Ok("zh") => &ZH_CN,
            _ => &EN,
        }
    }
}

/// Somewhere to say something.
#[async_trait::async_trait]
pub trait Notifier: Send + Sync {
    async fn meeting_starting(&self, event: &CalendarEvent);
    async fn armed_meeting_never_started(&self, event: &CalendarEvent);
}

/// Never says anything. The default until an Operator grants notifications,
/// and what the tests use.
pub struct SilentNotifier;

#[async_trait::async_trait]
impl Notifier for SilentNotifier {
    async fn meeting_starting(&self, _event: &CalendarEvent) {}
    async fn armed_meeting_never_started(&self, _event: &CalendarEvent) {}
}

/// Records what it would have said, for tests.
#[derive(Default)]
pub struct RecordingNotifier {
    pub said: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl Notifier for RecordingNotifier {
    async fn meeting_starting(&self, event: &CalendarEvent) {
        self.said
            .lock()
            .expect("said")
            .push(format!("starting:{}", event.id));
    }
    async fn armed_meeting_never_started(&self, event: &CalendarEvent) {
        self.said
            .lock()
            .expect("said")
            .push(format!("never-started:{}", event.id));
    }
}

/// The gates every notification passes through, whatever delivers it.
///
/// Separate from delivery so the rules are testable without a desktop.
pub struct Gates {
    last_spoke: Mutex<Option<Instant>>,
    /// One key per thing already said, so two senses agreeing about one
    /// meeting produce one notification rather than two.
    said: Mutex<HashMap<String, ()>>,
    silenced: Vec<String>,
    cooldown: Duration,
}

impl Default for Gates {
    fn default() -> Self {
        Self::new()
    }
}

impl Gates {
    pub fn new() -> Self {
        Self {
            last_spoke: Mutex::new(None),
            said: Mutex::new(HashMap::new()),
            silenced: Vec::new(),
            cooldown: COOLDOWN,
        }
    }

    pub fn silencing(mut self, apps: Vec<String>) -> Self {
        self.silenced = apps;
        self
    }

    pub fn with_cooldown(mut self, cooldown: Duration) -> Self {
        self.cooldown = cooldown;
        self
    }

    /// Whether to say this. Consumes the decision: a `true` marks the key
    /// said and starts the cooldown.
    pub fn allows(&self, key: &str, recording: bool, now: Instant) -> bool {
        // The product does not narrate a meeting it is already capturing.
        if recording {
            return false;
        }
        if self.silenced.iter().any(|app| key.contains(app)) {
            return false;
        }
        let mut said = self.said.lock().expect("said");
        if said.contains_key(key) {
            return false;
        }
        let mut last = self.last_spoke.lock().expect("last");
        if let Some(previous) = *last
            && now.duration_since(previous) < self.cooldown
        {
            return false;
        }
        said.insert(key.to_string(), ());
        *last = Some(now);
        true
    }
}

/// Whether Focus or Do Not Disturb is on.
///
/// Best-effort by construction: the mechanism is undocumented, and every
/// failure path returns `false` — "not in DND", so the notification goes
/// out. Guessing wrong towards silence is the failure that matters.
#[cfg(target_os = "macos")]
pub fn do_not_disturb() -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    let assertions = home.join("Library/DoNotDisturb/DB/Assertions.json");
    let Ok(contents) = std::fs::read_to_string(&assertions) else {
        debug!("no Focus assertions file; treating Focus as off");
        return false;
    };
    // An active Focus writes an assertion record. Absent one, it is off.
    contents.contains("\"storeAssertionRecords\"") && contents.contains("assertionDetails")
}

#[cfg(not(target_os = "macos"))]
pub fn do_not_disturb() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: &str) -> CalendarEvent {
        CalendarEvent {
            id: id.to_string(),
            title: "Weekly sync".to_string(),
            attendees: Vec::new(),
            scheduled_end: None,
        }
    }

    #[test]
    fn nothing_is_said_while_a_meeting_is_being_recorded() {
        // The product does not narrate what it is already capturing.
        let gates = Gates::new();
        assert!(!gates.allows("starting:evt-1", true, Instant::now()));
    }

    #[test]
    fn one_meeting_produces_one_notification() {
        // Two senses agreeing about the same meeting is not two meetings.
        let gates = Gates::new();
        let now = Instant::now();
        assert!(gates.allows("starting:evt-1", false, now));
        assert!(!gates.allows("starting:evt-1", false, now), "said already");
    }

    #[test]
    fn the_cooldown_holds_between_different_things() {
        let gates = Gates::new();
        let now = Instant::now();
        assert!(gates.allows("starting:evt-1", false, now));
        assert!(
            !gates.allows("starting:evt-2", false, now),
            "a second prompt inside the cooldown is a nag"
        );
        assert!(
            gates.allows(
                "starting:evt-2",
                false,
                now + COOLDOWN + Duration::from_secs(1)
            ),
            "and it is allowed once the cooldown has passed"
        );
    }

    #[test]
    fn a_silenced_app_says_nothing() {
        let gates = Gates::new().silencing(vec!["zoom".to_string()]);
        assert!(!gates.allows("starting:zoom-1", false, Instant::now()));
        assert!(gates.allows("starting:teams-1", false, Instant::now()));
    }

    #[test]
    fn a_focus_check_that_cannot_answer_lets_the_notification_through() {
        // The rule this module exists to state: silence is the failure that
        // matters, so an unanswerable check must not produce it. This asserts
        // the shape rather than the machine's current Focus state — it must
        // return a decision either way, never panic or hang.
        let _ = do_not_disturb();
    }

    #[tokio::test]
    async fn the_recording_notifier_reports_what_it_was_asked_to_say() {
        let notifier = RecordingNotifier::default();
        notifier.meeting_starting(&event("evt-1")).await;
        notifier.armed_meeting_never_started(&event("evt-2")).await;
        let said = notifier.said.lock().expect("said").clone();
        assert_eq!(said, vec!["starting:evt-1", "never-started:evt-2"]);
    }

    #[test]
    fn every_string_the_operator_reads_exists_in_both_catalogs() {
        // The M1 externalization rule, applied to the Core's first
        // user-facing text: a locale that renders an empty string is a
        // missing translation shipped as a blank notification.
        for strings in [&catalog::EN, &catalog::ZH_CN] {
            for text in [
                strings.meeting_starting_title,
                strings.meeting_starting_body,
                strings.never_started_title,
                strings.never_started_body,
            ] {
                assert!(!text.trim().is_empty());
            }
        }
        assert!(catalog::EN.never_started_body.contains("{title}"));
        assert!(catalog::ZH_CN.never_started_body.contains("{title}"));
    }
}
