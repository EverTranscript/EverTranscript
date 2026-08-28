//! The menu bar item: the Core's own face.
//!
//! ADR-0023 makes the Core a login item that is always running, which raises
//! a question the Electron client does not answer — how does an Operator see
//! that it is recording, and stop it, without opening an app? The tray is
//! that answer, and it is why the Core is a UI-capable agent rather than a
//! background daemon.
//!
//! **The state machine lives here, the platform code does not.** Everything
//! in this module is ordinary Rust with tests: which label the menu shows,
//! when the item is clickable, how a transition resolves, what happens when
//! a start fails. [`macos`] is a thin shell that renders a [`TrayView`] and
//! forwards clicks back. That split is deliberate — menu-bar code cannot be
//! asserted on by a test suite, so as little behaviour as possible lives
//! inside it.
//!
//! Two things the shape is chosen for:
//!
//! - **Transitional states are real states.** Recording starts by spawning
//!   work that takes a moment; a menu that still says "Start Recording"
//!   during it invites a second click, and one that flips to "Stop" before
//!   capture is running is lying. So `Starting` and `Stopping` are phases,
//!   they show immediately on click, and they end only when the Core agrees.
//! - **A failed start must not strand the menu.** Recording can be refused —
//!   an unacknowledged Briefing, no audio at all — and a tray stuck on
//!   "Starting…" forever would be worse than one that never moved. The
//!   outcome comes back and becomes the status line.

use std::sync::Arc;
use std::sync::Mutex;

use evertranscript_protocol::CoreState;
use tokio_util::sync::CancellationToken;
use tracing::info;
use tracing::warn;

use crate::Core;

#[cfg(target_os = "macos")]
mod macos;

/// How often the tray re-reads the Core.
///
/// Fast enough that stopping from the Electron client updates the menu bar
/// before the Operator looks at it, slow enough to be free.
const POLL: std::time::Duration = std::time::Duration::from_millis(500);

/// What the tray is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayPhase {
    /// No transcription model yet. Recording still works — the Core does
    /// not require one (ADR-0019), and refusing to record a meeting because
    /// captions are unavailable would lose the meeting to save the
    /// transcript. The tray says so rather than blocking.
    NotReady,
    /// The Briefing has not been acknowledged. Nothing is captured before
    /// that (ADR-0023), and the tray must not offer an action that would be
    /// refused.
    NotPermitted,
    Idle,
    Starting,
    Recording,
    Stopping,
}

/// Everything the menu bar shows at one moment.
#[derive(Debug, Clone, PartialEq)]
pub struct TrayView {
    /// The status item's own title — the always-visible indicator. Short,
    /// because it sits in the menu bar next to everything else.
    pub indicator: String,
    /// The record/stop item's title.
    pub action: String,
    /// Whether that item can be clicked.
    pub action_enabled: bool,
    /// A non-clickable line above it, explaining the current state.
    pub status: String,
}

impl TrayPhase {
    /// Whether clicking the action item should do anything.
    ///
    /// Transitions are deliberately not clickable: the work is already
    /// under way, and a second click would either start a second Meeting or
    /// stop one that has not begun. `NotPermitted` is the only state that
    /// truly cannot record, because it is the only one the Core itself
    /// refuses — the tray must not invent a stricter rule than the thing it
    /// is a face for.
    pub fn is_actionable(self) -> bool {
        matches!(
            self,
            TrayPhase::Idle | TrayPhase::Recording | TrayPhase::NotReady
        )
    }

    fn view(self, detail: Option<&str>) -> TrayView {
        let (indicator, action, status) = match self {
            // A dot rather than a word: the menu bar is shared real estate,
            // and a recording indicator that takes up half of it is one the
            // Operator turns off.
            TrayPhase::Recording => ("●", "Stop Recording", "Recording"),
            TrayPhase::Starting => ("○", "Starting…", "Starting the recording"),
            TrayPhase::Stopping => ("○", "Stopping…", "Finishing the recording"),
            TrayPhase::Idle => ("○", "Start Recording", "Ready"),
            TrayPhase::NotReady => (
                "○",
                "Start Recording",
                "No transcription model yet — this will record without captions",
            ),
            TrayPhase::NotPermitted => (
                "○",
                "Start Recording",
                "Blocked until the first-run briefing is acknowledged",
            ),
        };
        TrayView {
            indicator: indicator.to_string(),
            action: action.to_string(),
            action_enabled: self.is_actionable(),
            // A reported failure outranks the generic description: it is the
            // thing the Operator needs and it will not survive a poll.
            status: detail.unwrap_or(status).to_string(),
        }
    }
}

/// The tray's mutable state, shared between the main thread and the runtime.
#[derive(Debug)]
struct State {
    phase: TrayPhase,
    /// What went wrong last, shown until the next state change clears it.
    detail: Option<String>,
}

/// Drives the tray: reads the Core, applies clicks, publishes a view.
///
/// Platform-independent on purpose. `macos` renders what this produces and
/// calls into it; the tests below drive it with no menu bar at all.
pub struct TrayController {
    core: Arc<Core>,
    runtime: tokio::runtime::Handle,
    shutdown: CancellationToken,
    state: Mutex<State>,
}

impl TrayController {
    pub fn new(
        core: Arc<Core>,
        runtime: tokio::runtime::Handle,
        shutdown: CancellationToken,
    ) -> Arc<Self> {
        Arc::new(Self {
            core,
            runtime,
            shutdown,
            state: Mutex::new(State {
                phase: TrayPhase::Idle,
                detail: None,
            }),
        })
    }

    /// The view to render right now.
    pub fn view(&self) -> TrayView {
        let state = self.lock();
        state.phase.view(state.detail.as_deref())
    }

    pub fn phase(&self) -> TrayPhase {
        self.lock().phase
    }

    /// A lock that survives a panic elsewhere: a poisoned mutex must not
    /// take the menu bar down with it.
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Re-reads the Core and settles the phase against it.
    pub async fn refresh(&self) {
        let core_state = self.core.state().await;
        let acknowledged = self.core.briefing_acknowledged().await;
        // Model readiness touches the filesystem, so it is read here on the
        // runtime rather than on the main thread.
        let ready = self
            .core
            .models_status()
            .map(|status| status.ready)
            .unwrap_or(false);

        let mut state = self.lock();
        state.phase = match (state.phase, core_state) {
            // A transition ends when the Core agrees it has happened, not
            // when a timer says so.
            (TrayPhase::Starting, CoreState::Recording) => TrayPhase::Recording,
            (TrayPhase::Stopping, CoreState::Idle) => TrayPhase::Idle,
            // Until then it stands, so the menu does not flicker back to
            // the state the Operator just left.
            (TrayPhase::Starting, _) => TrayPhase::Starting,
            (TrayPhase::Stopping, _) => TrayPhase::Stopping,
            // Otherwise the Core is the truth. Another Client stopping a
            // Meeting is reflected here without the tray being told.
            (_, CoreState::Recording) => TrayPhase::Recording,
            (_, CoreState::Idle) if !acknowledged => TrayPhase::NotPermitted,
            (_, CoreState::Idle) if !ready => TrayPhase::NotReady,
            (_, CoreState::Idle) => TrayPhase::Idle,
        };
        // A settled state has nothing left to explain.
        if matches!(state.phase, TrayPhase::Recording | TrayPhase::Idle) {
            state.detail = None;
        }
    }

    /// The record/stop item was clicked.
    ///
    /// Returns the view to render immediately: the click has to show before
    /// the work finishes, or the menu bar feels broken.
    pub fn activate(self: &Arc<Self>) -> TrayView {
        let was = {
            let mut state = self.lock();
            let was = state.phase;
            match was {
                TrayPhase::Recording => state.phase = TrayPhase::Stopping,
                // Idle and NotReady both start: a missing model costs
                // captions, not the meeting. Keeping this in step with
                // `is_actionable` matters — an item that looks clickable and
                // does nothing is worse than one that is visibly disabled.
                TrayPhase::Idle | TrayPhase::NotReady => state.phase = TrayPhase::Starting,
                // A transition is already running. Clicking again must do
                // nothing, or it would start a second Meeting.
                _ => return state.phase.view(state.detail.as_deref()),
            }
            state.detail = None;
            was
        };

        let this = Arc::clone(self);
        let starting = this.phase() == TrayPhase::Starting;
        self.runtime.spawn(async move {
            let outcome = if starting {
                // No title and no detected app: this is the Operator saying
                // "record now", which is exactly the manual path.
                this.core.start_meeting(None, None).await.map(|_| ())
            } else {
                this.core.stop_meeting().await.map(|_| ())
            };
            if let Err(error) = outcome {
                warn!(%error, "the tray could not change recording state");
                let mut state = this.lock();
                // Back to exactly where it was, with the reason. A tray
                // stuck on "Starting…" would be worse than one that never
                // moved, and guessing at `Idle` would erase a NotReady the
                // next refresh would only have to restore.
                state.phase = was;
                state.detail = Some(first_line(&format!("{error:#}")));
            }
        });
        self.view()
    }

    /// Quit was chosen. The Core stops; nothing else does.
    pub fn quit(&self) {
        info!("quit chosen from the tray");
        self.shutdown.cancel();
    }

    pub fn shutdown_token(&self) -> &CancellationToken {
        &self.shutdown
    }
}

/// The first line of an error, for a menu item that has one line.
fn first_line(message: &str) -> String {
    let line = message.lines().next().unwrap_or(message).trim();
    // Menu items do not wrap, and the menu is as wide as its widest item.
    if line.chars().count() > 64 {
        let short: String = line.chars().take(61).collect();
        format!("{short}…")
    } else {
        line.to_string()
    }
}

/// Why the tray is not running, when it is not.
#[derive(Debug)]
pub enum Unavailable {
    /// No window server to put a menu bar item in — a CI runner, or an SSH
    /// session with nobody logged in at the machine.
    NoGuiSession,
    /// Switched off deliberately.
    Disabled,
    /// This build has no tray for this platform.
    Unsupported,
    /// The platform refused.
    Failed(String),
}

impl std::fmt::Display for Unavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unavailable::NoGuiSession => write!(formatter, "no GUI session to show a menu bar in"),
            Unavailable::Disabled => write!(formatter, "{DISABLE_ENV} is set"),
            Unavailable::Unsupported => write!(formatter, "no tray on this platform yet"),
            Unavailable::Failed(reason) => write!(formatter, "{reason}"),
        }
    }
}

/// Set this to run the Core with no menu bar item.
///
/// A headless deployment is a real deployment: the guarantee tests start the
/// Core dozens of times, and none of them want a menu bar.
pub const DISABLE_ENV: &str = "EVERTRANSCRIPT_NO_TRAY";

/// Runs the tray, blocking the calling thread until Quit.
///
/// **Must be called on the main thread**, which is what forces the daemon's
/// shape: the async runtime does the work on its own threads, and the main
/// thread belongs to the platform's event loop.
///
/// Returns `Err` when there is no tray to run, which is not a failure — the
/// caller serves headless instead.
pub fn run(controller: Arc<TrayController>) -> Result<(), Unavailable> {
    if std::env::var_os(DISABLE_ENV).is_some() {
        return Err(Unavailable::Disabled);
    }
    #[cfg(target_os = "macos")]
    {
        macos::run(controller)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = controller;
        Err(Unavailable::Unsupported)
    }
}

/// Keeps the published view in step with the Core.
///
/// Spawned on the runtime so the main thread never waits on a lock the
/// Core holds.
pub async fn poll(controller: Arc<TrayController>) {
    let shutdown = controller.shutdown.clone();
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return,
            _ = tokio::time::sleep(POLL) => controller.refresh().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recording_tray_offers_to_stop_and_says_so_in_the_menu_bar() {
        let view = TrayPhase::Recording.view(None);
        assert_eq!(view.action, "Stop Recording");
        assert!(view.action_enabled);
        assert_eq!(
            view.indicator, "●",
            "recording is visible without opening the menu"
        );
    }

    #[test]
    fn transitions_are_shown_and_cannot_be_clicked_again() {
        // A second click during a start would either open a second Meeting
        // or stop one that has not begun.
        for phase in [TrayPhase::Starting, TrayPhase::Stopping] {
            let view = phase.view(None);
            assert!(!view.action_enabled, "{phase:?} must not be clickable");
            assert!(
                view.action.ends_with('…'),
                "{phase:?} should read as in progress, got {:?}",
                view.action
            );
        }
    }

    #[test]
    fn a_missing_model_warns_without_refusing_to_record() {
        // ADR-0019: the Core records with no model, it just has no captions.
        // A tray that blocked here would lose the meeting to save the
        // transcript, which is backwards.
        let not_ready = TrayPhase::NotReady.view(None);
        assert!(
            not_ready.action_enabled,
            "recording must still be offered without a model"
        );
        assert!(
            not_ready.status.contains("without captions"),
            "and the Operator must know what they will not get, got {:?}",
            not_ready.status
        );
    }

    #[test]
    fn a_core_that_cannot_record_yet_says_why_rather_than_offering_the_action() {
        let blocked = TrayPhase::NotPermitted.view(None);
        assert!(!blocked.action_enabled);
        assert!(
            blocked.status.contains("briefing"),
            "the reason must be the actual gate, got {:?}",
            blocked.status
        );
    }

    #[test]
    fn a_reported_failure_replaces_the_generic_status() {
        let view = TrayPhase::Idle.view(Some("no audio can be captured"));
        assert_eq!(view.status, "no audio can be captured");
        assert!(view.action_enabled, "and the Operator can still try again");
    }

    #[test]
    fn a_long_error_is_cut_to_something_a_menu_can_show() {
        let sprawling = format!("{}\nand a second line", "x".repeat(200));
        let line = first_line(&sprawling);
        assert!(
            line.chars().count() <= 64,
            "got {} chars",
            line.chars().count()
        );
        assert!(line.ends_with('…'));
        assert!(!line.contains('\n'), "a menu item is one line");
    }

    #[test]
    fn a_short_error_is_left_alone() {
        assert_eq!(first_line("no microphone"), "no microphone");
    }
}
