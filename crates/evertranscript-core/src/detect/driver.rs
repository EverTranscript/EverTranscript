//! Auto-Record, running.
//!
//! The policy decides and the Core acts; this is the thin thing between
//! them. It owns a [`DetectionSource`], feeds every event to an
//! [`AutoRecord`], and turns the actions into Meetings — plus the one piece
//! of state neither of them can see on their own: whether the Operator has
//! stopped a recording by hand since the last event.

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::debug;
use tracing::info;
use tracing::warn;

use super::DetectionEvent;
use super::DetectionSource;
use super::notify::Notifier;
use super::policy::Action;
use super::policy::AutoRecord;
use crate::server::Core;

/// How many detection events to buffer. A poll produces a handful a second;
/// this is only about never blocking the detector thread.
const CHANNEL_CAPACITY: usize = 256;

/// Drives Auto-Record until shutdown.
pub async fn run(
    core: Arc<Core>,
    mut source: Box<dyn DetectionSource>,
    notifier: Box<dyn Notifier>,
    shutdown: CancellationToken,
) {
    let watchlist = match core.watchlist_for_detection().await {
        Ok(list) => list,
        Err(error) => {
            warn!(%error, "could not read the Watchlist; Auto-Record is not running");
            return;
        }
    };
    let mut policy = AutoRecord::new(watchlist);

    let (events_tx, mut events_rx) = mpsc::channel::<DetectionEvent>(CHANNEL_CAPACITY);
    if let Err(error) = source.start(events_tx) {
        warn!(%error, source = source.describe(), "detection could not start");
        return;
    }
    info!(source = source.describe(), "Meeting Detection is watching");

    // What the policy believed last time round, so a Meeting that vanished
    // between events can be recognised as the Operator's own Stop.
    let mut policy_thought_recording = false;

    loop {
        let event = tokio::select! {
            _ = shutdown.cancelled() => break,
            event = events_rx.recv() => match event {
                Some(event) => event,
                None => break,
            },
        };

        // Settings are read per event rather than cached: the single
        // Auto-Record switch has to take effect when it is flipped, not at
        // the next restart.
        let settings = core.settings().await;
        policy.set_enabled(settings.auto_record);
        policy.set_acknowledged(settings.briefing_acknowledged);

        // The Operator pressing Stop is invisible to the policy — it sees
        // the machine, and a person is not part of the machine. If the
        // policy believes it is recording and the Core is not, that gap is
        // the Operator, and their Stop must win (story 11).
        if policy_thought_recording && !core.is_recording().await {
            debug!("the Operator stopped a detected Meeting; suppressing re-trigger");
            policy.stopped_by_operator();
        }

        for action in policy.on_event(&event) {
            act(&core, &*notifier, action).await;
        }
        policy_thought_recording = policy.is_recording();
    }

    source.stop();
    info!("Meeting Detection stopped");
}

async fn act(core: &Arc<Core>, notifier: &dyn Notifier, action: Action) {
    match action {
        Action::StartRecording { app, armed } => {
            // The title chain (ADR-0030 as amended): the calendar names the
            // Meeting when it armed one, and the detected app is the
            // placeholder otherwise.
            let title = armed.as_ref().map(|event| event.title.clone());
            match core.start_meeting(title, Some(app.name.clone())).await {
                Ok(meeting) => info!(
                    meeting = meeting.id,
                    app = app.id,
                    "Auto-Record started a Meeting"
                ),
                // Never fatal: the commonest reason is the Briefing, and a
                // Core that died because a meeting could not start would be
                // worse than one that missed it.
                Err(error) => warn!(%error, app = app.id, "Auto-Record could not start a Meeting"),
            }
        }
        Action::StopRecording => match core.stop_meeting().await {
            Ok(meeting) => info!(meeting = meeting.id, "Auto-Record stopped a Meeting"),
            Err(error) => debug!(%error, "nothing to stop"),
        },
        Action::ArmForCalendarEvent { event } => {
            info!(
                event = event.id,
                title = event.title,
                "armed by the calendar"
            );
            notifier.meeting_starting(&event).await;
        }
        Action::ArmedMeetingNeverStarted { event } => {
            notifier.armed_meeting_never_started(&event).await;
        }
    }
}
