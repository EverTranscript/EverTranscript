//! Auto-Record: the decision to start, and the harder decision to stop.
//!
//! A pure state machine. It is handed a timeline and returns what should
//! happen; it never reads a clock, spawns a task, or touches a Meeting. That
//! is what makes "does Auto-Record behave" answerable from a fixture instead
//! of from a real meeting — and the cases that cost a Meeting when they are
//! wrong (a device swap that splits a recording, a manual Stop the machine
//! overrules) are exactly the cases nobody can hold on demand.
//!
//! **Where the debounces are.** The absorption catalog puts a ~500 ms edge
//! debounce and a ~2 s mic-list-empty debounce in the *detector*, and they
//! stay there (tickets 04 and 05). Repeating them here would compose with
//! the continuity window rather than reinforce it — a 2 s debounce in front
//! of a 15 s window is a 17 s window, arrived at by accident. This module
//! owns the window; the detectors own the smoothing that stops them
//! reporting a flap as an event at all.

use super::AppIdentity;
use super::CalendarEvent;
use super::DetectionEvent;
use super::DetectionInstant;
use super::watchlist::Watchlist;
use std::collections::BTreeMap;
use std::collections::BTreeSet;

/// How long the microphone can be quiet before the meeting is over.
///
/// ADR-0023 as amended: a window, not an instant, because a Bluetooth swap
/// mid-call releases the microphone for several seconds and that must
/// continue the *same* Meeting rather than ending one and starting another.
pub const CONTINUITY_WINDOW_MS: u64 = 15_000;

/// How long after a scheduled start a calendar-armed meeting waits for a
/// real trigger before the Operator is asked about it once (ADR-0036).
pub const ARMED_FOLLOW_UP_MS: u64 = 120_000;

/// Tunables, so a test can compress an afternoon without editing constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyConfig {
    pub continuity_window_ms: u64,
    pub armed_follow_up_ms: u64,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            continuity_window_ms: CONTINUITY_WINDOW_MS,
            armed_follow_up_ms: ARMED_FOLLOW_UP_MS,
        }
    }
}

/// What the Core should do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Begin recording, attributed to this app. Carries the armed calendar
    /// event when one named this meeting in advance.
    StartRecording {
        app: AppIdentity,
        armed: Option<CalendarEvent>,
    },
    /// End the Meeting: the continuity window expired with the microphone
    /// still quiet.
    StopRecording,
    /// A scheduled meeting has started; pre-arm and say so (ADR-0036).
    ArmForCalendarEvent { event: CalendarEvent },
    /// A scheduled meeting never produced a trigger. Asked once, then the
    /// pre-created Meeting is discarded.
    ArmedMeetingNeverStarted { event: CalendarEvent },
}

/// Why the policy is not currently recording.
#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    /// Nothing is happening.
    Idle,
    /// A Meeting is being recorded, triggered by this app.
    Recording { app: AppIdentity },
    /// The microphone went quiet at `since`; stopping unless it comes back
    /// (ADR-0023 as amended).
    Closing {
        app: AppIdentity,
        since: DetectionInstant,
    },
    /// The Operator stopped this one by hand. Detection must not overrule
    /// them for the rest of *this* meeting (story 11).
    Suppressed { app: AppIdentity },
    /// The Operator stopped by hand, and the meeting has since ended — but
    /// nothing new has started yet. Distinct from `Idle` only in that it
    /// remembers nothing; kept as a state so the transition is explicit.
    Released,
}

/// The standing policy that detection starts and stops recording.
pub struct AutoRecord {
    watchlist: Watchlist,
    config: PolicyConfig,
    /// The single visible switch (ADR-0023). Off means this decides nothing.
    enabled: bool,
    /// Nothing is captured before the Briefing acknowledgment (ADR-0023).
    /// Unchanged from M1: an ambient trigger does not weaken a pre-capture
    /// invariant.
    acknowledged: bool,
    state: State,
    /// Who currently holds the microphone, by responsible app.
    mic_holders: BTreeSet<String>,
    /// Calendar events that have started and not yet been resolved.
    armed: BTreeMap<String, ArmedEvent>,
}

#[derive(Debug, Clone)]
struct ArmedEvent {
    event: CalendarEvent,
    at: DetectionInstant,
    followed_up: bool,
    consumed: bool,
}

impl AutoRecord {
    pub fn new(watchlist: Watchlist) -> Self {
        Self::with_config(watchlist, PolicyConfig::default())
    }

    pub fn with_config(watchlist: Watchlist, config: PolicyConfig) -> Self {
        Self {
            watchlist,
            config,
            enabled: true,
            acknowledged: true,
            state: State::Idle,
            mic_holders: BTreeSet::new(),
            armed: BTreeMap::new(),
        }
    }

    /// The single Auto-Record switch.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Whether the Briefing has been acknowledged on this machine.
    pub fn set_acknowledged(&mut self, acknowledged: bool) {
        self.acknowledged = acknowledged;
    }

    pub fn set_watchlist(&mut self, watchlist: Watchlist) {
        self.watchlist = watchlist;
    }

    pub fn is_recording(&self) -> bool {
        matches!(self.state, State::Recording { .. } | State::Closing { .. })
    }

    /// The Operator pressed Stop while a detected Meeting was recording.
    ///
    /// Suppression lasts until the meeting it belongs to is over — see
    /// [`Self::trigger_present`] for what ends it. A timer alone would
    /// either overrule the Operator (too short) or silently miss the next
    /// meeting in the same app (too long); the meeting's own end is the only
    /// honest boundary.
    pub fn stopped_by_operator(&mut self) {
        if let State::Recording { app } | State::Closing { app, .. } = self.state.clone() {
            self.state = State::Suppressed { app };
        }
    }

    /// Feeds one detection event and returns what should happen.
    pub fn on_event(&mut self, event: &DetectionEvent) -> Vec<Action> {
        self.track(event);

        // Off, or not yet permitted: observe, decide nothing. Tracking still
        // runs so that turning the switch back on mid-meeting sees the truth
        // rather than an empty world.
        if !self.enabled || !self.acknowledged {
            return Vec::new();
        }

        let now = event.at();
        let mut actions = self.calendar_actions(event, now);
        actions.extend(self.recording_actions(now));
        actions
    }

    /// Updates what is true about the machine, regardless of policy.
    fn track(&mut self, event: &DetectionEvent) {
        match event {
            DetectionEvent::MicHeld { app, .. } => {
                self.mic_holders.insert(app.id.clone());
            }
            DetectionEvent::MicReleased { app, .. } => {
                self.mic_holders.remove(&app.id);
            }
            DetectionEvent::AppGone { app, .. } => {
                // An app that exited is not holding anything.
                self.mic_holders.remove(&app.id);
            }
            _ => {}
        }
    }

    /// The app that should be recorded, if any: watched, and holding the
    /// microphone.
    ///
    /// ADR-0024 asks for Watchlist membership AND microphone use, and
    /// requiring them of the *same* app is what makes both of its exclusions
    /// fall out: an idle Zoom window holds no microphone, and a hot
    /// microphone in a dictation app belongs to something not on the list.
    /// It also gives the latch for free — an app moved to the background
    /// keeps recording, because being frontmost was never the condition.
    fn trigger_present(&self) -> Option<AppIdentity> {
        self.mic_holders
            .iter()
            .map(|id| AppIdentity::bare(id))
            .find(|app| self.watchlist.watches(app))
    }

    fn recording_actions(&mut self, now: DetectionInstant) -> Vec<Action> {
        let mut actions = Vec::new();

        // A window that expired during a silence expired *then*, not now.
        //
        // Without this the policy is only correct while something keeps
        // asking it questions: half an hour of quiet between two meetings
        // arrives as a single event, the window is still nominally open when
        // it does, and the microphone coming back reads as a device swap —
        // so two meetings half an hour apart become one Meeting with a
        // thirty-minute hole in it. Deciding at the deadline rather than at
        // the next interruption makes the outcome independent of how often
        // the source happens to speak.
        if let State::Closing { since, .. } = self.state.clone()
            && now.since(since) >= self.config.continuity_window_ms
        {
            self.state = State::Idle;
            actions.push(Action::StopRecording);
        }

        let trigger = self.trigger_present();
        actions.extend(match (self.state.clone(), trigger) {
            // Nothing happening, and something started.
            (State::Idle | State::Released, Some(app)) => {
                let armed = self.claim_armed_event();
                self.state = State::Recording { app: app.clone() };
                vec![Action::StartRecording { app, armed }]
            }

            // Recording, and still triggered: nothing to do.
            (State::Recording { .. }, Some(_)) => Vec::new(),

            // Recording, and the microphone just went quiet. Do not stop —
            // open the continuity window.
            (State::Recording { app }, None) => {
                self.state = State::Closing { app, since: now };
                Vec::new()
            }

            // The window is open and the microphone came back. This is the
            // device swap, and it must continue the same Meeting.
            (State::Closing { .. }, Some(app)) => {
                self.state = State::Recording { app };
                Vec::new()
            }

            // The window is open, still quiet, and not yet expired —
            // expiry was already handled above. Wait.
            (State::Closing { .. }, None) => Vec::new(),

            // Suppressed, and the meeting is still going. The Operator's
            // Stop stands.
            (State::Suppressed { .. }, Some(_)) => Vec::new(),

            // Suppressed, and the meeting ended. *This* is what lifts
            // suppression: the same evidence that would have ended the
            // Meeting normally. The next meeting in the same app records.
            (State::Suppressed { .. }, None) => {
                self.state = State::Released;
                Vec::new()
            }

            (State::Idle | State::Released, None) => Vec::new(),
        });
        actions
    }

    fn calendar_actions(&mut self, event: &DetectionEvent, now: DetectionInstant) -> Vec<Action> {
        let mut actions = Vec::new();
        match event {
            DetectionEvent::CalendarEventStarted { event, .. } => {
                self.armed.insert(
                    event.id.clone(),
                    ArmedEvent {
                        event: event.clone(),
                        at: now,
                        followed_up: false,
                        consumed: false,
                    },
                );
                actions.push(Action::ArmForCalendarEvent {
                    event: event.clone(),
                });
            }
            DetectionEvent::CalendarEventEnded { id, .. } => {
                self.armed.remove(id);
            }
            _ => {}
        }

        // A scheduled meeting that never produced a trigger: ask once.
        if !self.is_recording() {
            for armed in self.armed.values_mut() {
                if armed.followed_up || armed.consumed {
                    continue;
                }
                if now.since(armed.at) >= self.config.armed_follow_up_ms {
                    armed.followed_up = true;
                    actions.push(Action::ArmedMeetingNeverStarted {
                        event: armed.event.clone(),
                    });
                }
            }
        }
        actions
    }

    /// The armed event a starting Meeting should be named from, if one is
    /// waiting. Consumed so a second Meeting does not inherit the title.
    fn claim_armed_event(&mut self) -> Option<CalendarEvent> {
        self.armed
            .values_mut()
            .find(|armed| !armed.consumed)
            .map(|armed| {
                armed.consumed = true;
                armed.event.clone()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::fixture::Timeline;
    use crate::detect::fixture::timelines;

    /// Replays a timeline and returns everything the policy decided.
    fn run(policy: &mut AutoRecord, events: Vec<DetectionEvent>) -> Vec<Action> {
        events.iter().flat_map(|e| policy.on_event(e)).collect()
    }

    /// Replays the same timeline both ways — sparse, and with the quiet
    /// between events filled in. A policy that only holds for one of those
    /// is a policy that only holds against a fixture.
    fn both_ways(timeline: Timeline) -> [Vec<Action>; 2] {
        let sparse = run(
            &mut AutoRecord::new(Watchlist::shipped()),
            timeline.clone().into_events(),
        );
        let fragmented = run(
            &mut AutoRecord::new(Watchlist::shipped()),
            timeline.fragmented(1_000),
        );
        assert_eq!(
            sparse, fragmented,
            "the same meeting decided differently depending on how finely it was reported"
        );
        [sparse, fragmented]
    }

    fn starts(actions: &[Action]) -> usize {
        actions
            .iter()
            .filter(|a| matches!(a, Action::StartRecording { .. }))
            .count()
    }

    fn stops(actions: &[Action]) -> usize {
        actions
            .iter()
            .filter(|a| **a == Action::StopRecording)
            .count()
    }

    #[test]
    fn a_watchlist_app_with_a_hot_microphone_records() {
        let [actions, _] = both_ways(timelines::clean_meeting());
        assert_eq!(starts(&actions), 1, "one meeting, one recording");
        assert_eq!(stops(&actions), 1, "and it ended");
    }

    #[test]
    fn an_idle_window_records_nothing() {
        // ADR-0024's first exclusion: an idle Zoom window must not record
        // the office all day.
        let [actions, _] = both_ways(timelines::app_active_but_silent());
        assert_eq!(starts(&actions), 0, "no microphone, no meeting");
    }

    #[test]
    fn a_hot_microphone_alone_records_nothing() {
        // ADR-0024's second exclusion: dictation is not a meeting.
        let [actions, _] = both_ways(timelines::mic_held_by_stranger());
        assert_eq!(starts(&actions), 0, "not on the Watchlist, not a meeting");
    }

    #[test]
    fn a_device_swap_does_not_split_the_meeting() {
        // The expensive one. Eight seconds of silence inside a fifteen
        // second window is an AirPods swap, not the end of a call — and
        // ADR-0023 was amended precisely so this stays one Meeting.
        let [actions, _] = both_ways(timelines::device_swap_mid_meeting());
        assert_eq!(starts(&actions), 1, "one Meeting, not two");
        assert_eq!(stops(&actions), 1);
    }

    #[test]
    fn joining_a_meeting_already_under_way_records_the_remainder() {
        // Story 12: a partial record beats no record. Detection coming
        // online mid-meeting sees the microphone already held.
        let [actions, _] = both_ways(timelines::joined_late());
        assert_eq!(starts(&actions), 1);
    }

    #[test]
    fn the_second_meeting_of_the_day_in_the_same_app_still_records() {
        let [actions, _] = both_ways(timelines::back_to_back_meetings());
        assert_eq!(starts(&actions), 2, "two meetings, two recordings");
        assert_eq!(stops(&actions), 2);
    }

    #[test]
    fn a_long_silence_ends_the_meeting_even_if_nobody_asks() {
        // The bug the fragment rule found on its first use, and the reason
        // that rule is in ticket 01 at all. Two meetings half an hour apart
        // arrive as five events; between the microphone going quiet and
        // coming back, nothing asks the policy anything. Deciding the
        // deadline only when interrupted merged them into one Meeting with a
        // thirty-minute hole, and the sparse timeline showed one recording
        // where the fragmented one showed two.
        let mut policy = AutoRecord::new(Watchlist::shipped());
        let actions = run(
            &mut policy,
            Timeline::new()
                .mic_held("us.zoom.xos")
                .wait(600_000)
                .mic_released("us.zoom.xos")
                // Nothing at all for half an hour.
                .wait(1_800_000)
                .mic_held("us.zoom.xos")
                .into_events(),
        );
        assert_eq!(
            starts(&actions),
            2,
            "the second meeting is a second Meeting, however quiet the gap"
        );
        assert_eq!(stops(&actions), 1, "and the first one ended when it ended");
    }

    #[test]
    fn a_manual_stop_is_not_overruled_for_the_rest_of_that_meeting() {
        // Story 11: the machine never overrules the Operator. The
        // microphone stays hot the whole time — every tick is a chance for
        // the policy to change its mind, and it must not.
        let mut policy = AutoRecord::new(Watchlist::shipped());
        let start = Timeline::new()
            .app_active("us.zoom.xos")
            .mic_held("us.zoom.xos")
            .into_events();
        assert_eq!(starts(&run(&mut policy, start)), 1);

        policy.stopped_by_operator();

        let rest = Timeline::new().wait(600_000).fragmented(1_000);
        let actions = run(&mut policy, rest);
        assert_eq!(
            starts(&actions),
            0,
            "detection restarted a recording the Operator had stopped"
        );
    }

    #[test]
    fn suppression_ends_with_the_meeting_rather_than_on_a_timer() {
        // The ambiguity ticket 03 asked to settle. Too short and the
        // Operator is overruled; too long and the next meeting in the same
        // app is silently missed. The meeting's own end is the boundary.
        let mut policy = AutoRecord::new(Watchlist::shipped());
        run(
            &mut policy,
            Timeline::new()
                .app_active("us.zoom.xos")
                .mic_held("us.zoom.xos")
                .into_events(),
        );
        policy.stopped_by_operator();

        // Still in the meeting: suppressed.
        let during = run(&mut policy, Timeline::new().wait(300_000).fragmented(1_000));
        assert_eq!(starts(&during), 0, "still the same meeting");

        // The meeting ends, and a new one starts later.
        let after = run(
            &mut policy,
            Timeline::new()
                .mic_released("us.zoom.xos")
                .wait(1_800_000)
                .mic_held("us.zoom.xos")
                .fragmented(1_000),
        );
        assert_eq!(
            starts(&after),
            1,
            "the next meeting must record — suppression is not permanent"
        );
    }

    #[test]
    fn the_switch_off_means_the_policy_decides_nothing() {
        // ADR-0023's single visible switch. Off is off.
        let mut policy = AutoRecord::new(Watchlist::shipped());
        policy.set_enabled(false);
        let actions = run(&mut policy, timelines::clean_meeting().fragmented(1_000));
        assert!(actions.is_empty(), "Auto-Record was off, got {actions:?}");
    }

    #[test]
    fn nothing_is_decided_before_the_briefing_is_acknowledged() {
        // The M1 pre-capture invariant, unweakened by an ambient trigger.
        let mut policy = AutoRecord::new(Watchlist::shipped());
        policy.set_acknowledged(false);
        let actions = run(&mut policy, timelines::clean_meeting().fragmented(1_000));
        assert!(
            actions.is_empty(),
            "captured before consent, got {actions:?}"
        );
    }

    #[test]
    fn turning_the_switch_on_mid_meeting_sees_the_meeting_already_happening() {
        // The policy keeps tracking while it is off, so enabling it does not
        // require the Operator to leave and rejoin the call.
        let mut policy = AutoRecord::new(Watchlist::shipped());
        policy.set_enabled(false);
        run(
            &mut policy,
            Timeline::new()
                .app_active("us.zoom.xos")
                .mic_held("us.zoom.xos")
                .into_events(),
        );
        policy.set_enabled(true);
        let actions = run(
            &mut policy,
            Timeline::new().wait(1_000).tick().into_events(),
        );
        assert_eq!(starts(&actions), 1, "the meeting in progress should record");
    }

    #[test]
    fn a_calendar_event_arms_and_names_but_never_triggers() {
        // ADR-0036: the calendar knows when, only the microphone knows that.
        let mut policy = AutoRecord::new(Watchlist::shipped());
        let armed = run(
            &mut policy,
            Timeline::new()
                .calendar_started("evt-1", "Weekly sync", 1_800_000)
                .wait(30_000)
                .fragmented(1_000),
        );
        assert!(
            armed
                .iter()
                .any(|a| matches!(a, Action::ArmForCalendarEvent { .. })),
            "the event should arm"
        );
        assert_eq!(starts(&armed), 0, "arming must never start a recording");

        // The trigger arrives, and the Meeting is named from the event.
        let started = run(
            &mut policy,
            Timeline::new().mic_held("us.zoom.xos").into_events(),
        );
        match started.first() {
            Some(Action::StartRecording {
                armed: Some(event), ..
            }) => {
                assert_eq!(event.title, "Weekly sync");
            }
            other => panic!("expected a Meeting named from the calendar, got {other:?}"),
        }
    }

    #[test]
    fn an_armed_meeting_that_never_happens_is_raised_once() {
        // ADR-0036's follow-up: asked once, not nagged.
        let mut policy = AutoRecord::new(Watchlist::shipped());
        let actions = run(
            &mut policy,
            timelines::armed_but_never_triggered().fragmented(1_000),
        );
        let raised = actions
            .iter()
            .filter(|a| matches!(a, Action::ArmedMeetingNeverStarted { .. }))
            .count();
        assert_eq!(raised, 1, "asked exactly once, got {raised}");
        assert_eq!(starts(&actions), 0, "and nothing was recorded");
    }

    #[test]
    fn an_armed_meeting_that_does_happen_is_never_raised() {
        let mut policy = AutoRecord::new(Watchlist::shipped());
        let actions = run(
            &mut policy,
            Timeline::new()
                .calendar_started("evt-1", "Weekly sync", 1_800_000)
                .wait(10_000)
                .mic_held("us.zoom.xos")
                .wait(600_000)
                .fragmented(1_000),
        );
        assert!(
            !actions
                .iter()
                .any(|a| matches!(a, Action::ArmedMeetingNeverStarted { .. })),
            "it did happen; asking about it is a bug"
        );
    }
}
