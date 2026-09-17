//! The message processor: one running Core, 0..N attached Clients.
//!
//! Connection handling follows the codex app-server shape (per-connection
//! `initialize` handshake, typed dispatch, broadcast fanout to initialized
//! connections) with our own method table. Work continues when zero Clients
//! are attached — that property is why the Core is a daemon rather than a
//! child process (ADR-0026).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use evertranscript_protocol::AudioChannel;
use evertranscript_protocol::BriefingResponse;
use evertranscript_protocol::CalendarAccessResponse;
use evertranscript_protocol::ClientNotification;
use evertranscript_protocol::ClientRequest;
use evertranscript_protocol::CoreState;
use evertranscript_protocol::CoreStateChangedParams;
use evertranscript_protocol::DiarizeRerun;
use evertranscript_protocol::DiarizeState;
use evertranscript_protocol::DiarizeStatusResponse;
use evertranscript_protocol::HistorySearchResponse;
use evertranscript_protocol::InitializeParams;
use evertranscript_protocol::InitializeResponse;
use evertranscript_protocol::JsonRpcError;
use evertranscript_protocol::JsonRpcMessage;
use evertranscript_protocol::JsonRpcResponse;
use evertranscript_protocol::Meeting;
use evertranscript_protocol::MeetingChangeKind;
use evertranscript_protocol::MeetingChangedParams;
use evertranscript_protocol::MeetingDeleteResponse;
use evertranscript_protocol::MeetingDetailResponse;
use evertranscript_protocol::MeetingExportResponse;
use evertranscript_protocol::MeetingListResponse;
use evertranscript_protocol::MeetingResponse;
use evertranscript_protocol::ModelAvailability;
use evertranscript_protocol::ModelState;
use evertranscript_protocol::ModelsStatusResponse;
use evertranscript_protocol::PostureClaim;
use evertranscript_protocol::PostureResponse;
use evertranscript_protocol::RequestId;
use evertranscript_protocol::ServerCapabilities;
use evertranscript_protocol::ServerInfo;
use evertranscript_protocol::ServerNotification;
use evertranscript_protocol::SettingsResponse;
use evertranscript_protocol::SettingsSetParams;
use evertranscript_protocol::Speaker;
use evertranscript_protocol::SpeakerChangedParams;
use evertranscript_protocol::SpeakerDetailResponse;
use evertranscript_protocol::SpeakerJoinPreview;
use evertranscript_protocol::SpeakerListResponse;
use evertranscript_protocol::SpeakerMeeting;
use evertranscript_protocol::SpeakerResponse;
use evertranscript_protocol::SpeakerSampleClip;
use evertranscript_protocol::SpeakerSampleResponse;
use evertranscript_protocol::StatusResponse;
use evertranscript_protocol::SummaryBackendOption;
use evertranscript_protocol::SummaryBackendsResponse;
use evertranscript_protocol::SummaryDataHandling;
use evertranscript_protocol::TrafficEntry;
use evertranscript_protocol::TranscriptCaptionsDroppedParams;
use evertranscript_protocol::TranscriptReassignResponse;
use evertranscript_protocol::TranscriptSegment;
use evertranscript_protocol::TranscriptSegmentAddedParams;
use evertranscript_protocol::TranscriptSnapshotResponse;
use evertranscript_protocol::TranscriptUnsubscribeResponse;
use evertranscript_protocol::WatchlistAddParams;
use evertranscript_protocol::WatchlistKind;
use evertranscript_protocol::WatchlistResponse;
use evertranscript_protocol::error_codes;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::debug;
use tracing::info;
use tracing::warn;

use crate::audio;
use crate::autostart;
use crate::mirror;
use crate::mirror::MirrorWriter;
use crate::models;
use crate::paths;
use crate::settings::Settings;
use crate::store::Store;
use crate::store::diarize_queue;
use crate::store::meetings;
use crate::summary;
use crate::transport::ConnectionId;
use crate::transport::TransportEvent;

/// The default page size for `meeting/list`.
const DEFAULT_LIST_LIMIT: u32 = 50;
const DEFAULT_SEARCH_LIMIT: u32 = 25;

/// The Core's own state, independent of any Client.
pub struct Core {
    started_at: Instant,
    history_dir: std::path::PathBuf,
    state: Mutex<CoreState>,
    /// Set once at startup if the History folder looked like an incomplete
    /// copy. Sticky on purpose: creating the missing store would otherwise
    /// erase the only evidence that the Operator's copy was partial, and
    /// they would never learn their audio and Voiceprints were left behind.
    incomplete_copy: Option<String>,
    store: Store,
    mirror: MirrorWriter,
    mirror_wake: Arc<Notify>,
    /// Nudges the Diarization worker when something joins the queue. The
    /// worker also wakes on a timer, so a missed notify costs a delay
    /// rather than a dropped Meeting.
    diarize_wake: Arc<Notify>,
    /// The recording in progress, if any. The Meeting owns it; capture
    /// streams inside it come and go (ADR-0029 as amended).
    recorder: Mutex<Option<audio::recorder::Recorder>>,
    /// Whether a Meeting is being recorded, as something a blocking thread
    /// can read.
    ///
    /// The same fact as `recorder.is_some()` and deliberately not a second
    /// opinion about it: [`Core::is_recording`] answers from here, and the
    /// two lines that install and take the recorder are the only writers.
    /// The reason it exists at all is that Diarization's models run on a
    /// `spawn_blocking` thread which cannot await an async `Mutex`, and the
    /// re-run has to stand down for a recording that starts *during* an
    /// inference pass, not only for one already under way when it began
    /// (ticket 12).
    recording: Arc<std::sync::atomic::AtomicBool>,
    /// How to open capture. Swapped in tests for the fixture source — the
    /// AudioSource seam the PRD names.
    source_factory: Mutex<SourceFactory>,
    /// Notifications the Core raises on its own — transcript segments above
    /// all. A broadcast channel rather than a direct call into the server so
    /// the recording path never has to know whether anyone is attached:
    /// capture continues at zero Clients (ADR-0026).
    notifications: broadcast::Sender<ServerNotification>,
    /// How to open transcription. Overridden in tests so the caption path
    /// can be driven without a 900 MB model.
    transcriber_factory: Mutex<Option<TranscriberFactory>>,
    /// A std mutex rather than a tokio one: `backends` is sync, and making it
    /// async to read a test override would push `.await` through a call path
    /// that has no other reason for it.
    summary_backend_factory: std::sync::Mutex<Option<SummaryBackendFactory>>,
    /// Held for a whole Summary run, so runs go one at a time: a second
    /// local run would load a second copy of the model beside the first.
    summarizing: Mutex<()>,
    /// Cancels the model fetch in flight, if there is one.
    ///
    /// Held by the Core rather than made per-call, because the Client that
    /// wants to stop a download is not the one that started it — on a fresh
    /// install nobody started it, the binary did.
    fetching: std::sync::Mutex<Option<CancellationToken>>,
    /// This installation's settings, including the Briefing acknowledgment
    /// that gates all capture.
    settings: Mutex<Settings>,
    /// Where settings are stored. Overridable so tests never touch the real
    /// machine's acknowledgment state.
    settings_path: std::path::PathBuf,
    /// Where transcription models live. Overridable for the same reason as
    /// `settings_path`: read from the machine, a test's result depends on
    /// whether whoever ran the app here happened to fetch a model, and a
    /// suite that is fast and silent on one laptop runs real inference on
    /// the next.
    models_dir: std::path::PathBuf,
    /// The Diarization running now, if any.
    ///
    /// At most one: the catalog's batch policy is reject-don't-queue, and M1
    /// already paid for the version of this where transcription starved
    /// capture (DECISIONS Q7). Post-meeting work is the lowest-priority
    /// thing this process does.
    diarization: Mutex<Option<DiarizeJob>>,
}

/// A Diarization in progress.
#[derive(Debug, Clone)]
pub struct DiarizeJob {
    pub meeting_id: String,
    pub cancel: crate::diarize::Cancel,
    pub done_ms: u64,
    pub total_ms: u64,
}

/// What the blocking half of a run handed back: the diarizer's own answer,
/// and the Voiceprints it re-embedded on the way past (Q115).
type RunResult = std::result::Result<Ran, anyhow::Error>;

/// What one inference pass produced, before anything is written.
struct Ran {
    result: std::result::Result<crate::diarize::Diarization, crate::diarize::DiarizeError>,
    rebuilt: Vec<(String, Option<Vec<f32>>)>,
    /// Bounded evidence for a bulk re-run, prepared with the same embedder
    /// the run itself used. `None` for every ordinary run.
    ///
    /// `Err` is a preparation that failed — a recording that could not be
    /// read, a model that would not run. **Not folded into `None`**: nothing
    /// to relearn and could-not-relearn have to reach the writer as different
    /// answers, or a Meeting whose audio was briefly unreadable is walked,
    /// counted done, and quietly loses the voices it was queued to recover.
    prepared: Option<std::result::Result<Prepared, String>>,
}

/// One Meeting's re-seeding, computed outside any transaction.
struct Prepared {
    plan: crate::diarize::reseed::Plan,
    /// Parallel to `plan.ranges`.
    vectors: Vec<Option<Vec<f32>>>,
}

/// What a Diarization run did, as the queue worker has to read it.
///
/// `Ok(0)` used to carry three unrelated answers — nothing to attribute, a
/// model missing, the Operator cancelling — and the worker took the Meeting
/// out of the line for every one of them. Standing down for a recording is
/// the case that made that lossy: work still owed has to keep its turn, and
/// a count of zero cannot say whether it is owed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiarizeOutcome {
    /// The attribution transaction committed, and the count is what it
    /// attributed. It took the Meeting out of the queue as part of itself,
    /// so nothing afterwards has to remove the row — and nothing may, since
    /// by then a row under that id could be a fresh request somebody made in
    /// between.
    Wrote(usize),
    /// Nothing ran and nothing was written: no audio, no models, or a run
    /// that failed on its own recording. The Meeting still leaves the line,
    /// because the next pass would fail in exactly the same way — but it
    /// leaves it afterwards, since there was no transaction to leave it in.
    Skipped,
    /// Stood down for a recording without writing anything. Still owed.
    Paused,
    /// Stopped by the Operator or a disconnecting Client without writing
    /// anything. `diarize/cancel` takes the queue row out itself, so this
    /// answer leaves the row alone rather than deleting what is already gone.
    Cancelled,
    /// A bulk re-run could not relearn this Meeting's voices — the evidence
    /// would not embed, or the record moved under it while the model ran —
    /// and the transaction was dropped rather than committed. **Nothing was
    /// written, and the Meeting is still owed**, so the row stays.
    ///
    /// Apart from `Skipped` because the next pass can succeed where this one
    /// did not, and apart from `Paused` because nobody is recording: reported
    /// as a pause it would tell an Operator the machine is waiting for a call
    /// that is not happening.
    Owed,
}

/// Passes a Meeting still owed gets before it is given up on.
const RESEED_ATTEMPTS: u8 = 2;

/// What the attribution transaction did.
///
/// Three answers, not two: a stop found waiting and a re-seeding whose plan
/// no longer matches the record both write nothing, and the queue has to tell
/// them apart. A stop has already taken the row out; a Meeting still owed
/// keeps it.
enum Committed {
    Wrote(usize),
    Stopped,
    Owed,
}

/// Whether background Diarization has to stand down right now.
///
/// Only `Back` work yields. A Meeting that just ended, or one the Operator
/// asked for by hand, keeps its priority even while the next Meeting records:
/// somebody is waiting for that one, and Auto-Record starting a second call
/// is not a reason to make them wait longer. `Back` work is the overnight
/// re-run and the catch-up of what a previous Core left, and neither has
/// anybody waiting.
fn yields_to_recording(priority: crate::store::diarize_queue::Priority, recording: bool) -> bool {
    recording && matches!(priority, crate::store::diarize_queue::Priority::Back)
}

/// Whether the worker still has to take the Meeting out of the line.
///
/// Only for a run that never reached a transaction. A committed one removed
/// its own row inside the commit, and removing it again afterwards could
/// delete a fresh request somebody made in between — the row under that id is
/// no longer the one this run was working. A pause left it owed on purpose,
/// and a cancellation, single or bulk, already took it out with the re-run's
/// books settled in the same transaction.
fn leaves_the_line_afterwards(outcome: DiarizeOutcome) -> bool {
    matches!(outcome, DiarizeOutcome::Skipped)
}

/// Stops a run because a Meeting is recording, and records *that* as the
/// reason. Answers whether it stopped anything.
///
/// The reason has to be written down when the token is set, not worked out
/// afterwards from whether a Meeting happens to be recording by then. A
/// recording can start and end inside one inference pass, and then asking
/// `is_recording` on the way out reports an Operator cancellation that never
/// happened — losing the one fact the queue needs, which is that the Meeting
/// is still owed.
///
/// `recording: None` is a run that does not yield at all, so nothing can stand
/// it down.
fn stand_down_for_recording(
    recording: Option<&Arc<std::sync::atomic::AtomicBool>>,
    cancel: &crate::diarize::Cancel,
    stood_down: &Arc<std::sync::atomic::AtomicBool>,
) -> bool {
    let Some(recording) = recording else {
        return false;
    };
    if !recording.load(std::sync::atomic::Ordering::SeqCst) {
        return false;
    }
    stood_down.store(true, std::sync::atomic::Ordering::SeqCst);
    cancel.cancel();
    true
}

/// A stored Speaker as the protocol shows it, with its appearance counts.
///
/// The counts are derived here rather than stored on the row, so they cannot
/// drift from the segments they describe.
fn speaker_to_wire(
    connection: &rusqlite::Connection,
    row: crate::store::speakers::Speaker,
) -> Result<Speaker> {
    let seen = crate::store::speakers::appearances(connection, &row.id)?;
    let has_sample = crate::store::speakers::sample_source(connection, &row.id)?.is_some();
    Ok(Speaker {
        id: row.id,
        display_name: row.display_name,
        is_operator: row.is_operator,
        has_voiceprint: row.has_voiceprint,
        confirmed: row.confirmed,
        forgotten: row.forgotten,
        voiceprint_model: row.voiceprint_model,
        meetings_seen_in: seen.meetings,
        first_seen_at: seen.first_seen_at,
        first_meeting_id: seen.first_meeting_id,
        first_meeting_title: seen.first_meeting_title,
        first_meeting_app: seen.first_meeting_app,
        last_heard_at: seen.last_heard_at,
        last_meeting_id: seen.last_meeting_id,
        has_sample,
        created_at: row.created_at,
    })
}

/// The Backend the Operator chose, and the local one to fall back to.
///
/// A named pair rather than an inline tuple, and the naming carries the
/// guarantee: the second element is **always** local. Nothing in the type
/// permits a cloud Backend to arrive as a fallback.
/// What one Summary run produced.
///
/// Richer than the Knob's own outcome because a chunked run has facts the
/// single-request path never had: how many chunks there were, and how many of
/// them the Backend could not serve.
struct SummaryRun {
    text: String,
    used: summary::BackendIdentity,
    fell_back_from: Option<String>,
    chunks: usize,
    failed_chunks: usize,
    /// Why the first refused chunk was refused, for the Operator.
    refusal: Option<String>,
    /// Action items taken out because they credited the unnamed-Speaker
    /// placeholder rather than a person.
    dropped_items: usize,
}

type ChosenBackends = (
    Box<dyn summary::Backend + 'static>,
    Option<Box<dyn summary::Backend + 'static>>,
);

/// Where the Summary sidecar lives.
///
/// Beside the Core, because that is how both are installed. Overridable so a
/// developer running from `cargo` finds the one they just built.
fn summarizer_binary() -> Option<std::path::PathBuf> {
    if let Ok(path) = std::env::var("EVERTRANSCRIPT_SUMMARIZER_BIN") {
        return Some(std::path::PathBuf::from(path));
    }
    let name = if cfg!(windows) {
        "evertranscript-summarizer.exe"
    } else {
        "evertranscript-summarizer"
    };
    let beside = std::env::current_exe().ok()?.parent()?.join(name);
    beside.exists().then_some(beside)
}

/// Produces a transcription engine for a new Meeting.
pub type TranscriberFactory =
    Arc<dyn Fn() -> Option<Box<dyn crate::asr::Transcriber>> + Send + Sync>;

/// How many Core-raised notifications to buffer. A subscriber that falls
/// this far behind loses the oldest, which is the lossy caption policy
/// ADR-0028 requires: degraded captions, never blocked capture.
const NOTIFICATION_CAPACITY: usize = 512;

/// Produces a capture source for a new Meeting.
pub type SourceFactory = Arc<dyn Fn() -> Box<dyn audio::AudioSource> + Send + Sync>;

/// Produces the Backends a Summary run will use: the chosen one, and the
/// local one to fall back to.
///
/// The same shape as [`TranscriberFactory`] and [`SourceFactory`], and for the
/// same reason: a test cannot drive a real Backend without half a gigabyte of
/// model or a network call, and the behaviour worth testing here — which
/// Backend answered, what a failed chunk does, what the record ends up
/// holding — is about everything *around* generation rather than generation
/// itself.
pub type SummaryBackendFactory = Arc<dyn Fn() -> ChosenBackends + Send + Sync>;

/// When a Meeting a killed Core left open should be dated to end.
///
/// Three candidates, and `now` is not one of them: dating it to whenever the
/// next Core happened to start is how a Meeting nobody attended acquires a
/// duration. What actually happened is bounded by two things, and the longer
/// one wins because both are lower bounds on a recording that really ran:
///
/// * **Recovered audio.** The encoder is CBR, so the file's length is the
///   duration it holds. A real 69-hour orphan is what made this the deciding
///   term:
///   its row was never touched after creation, so the timestamp below said
///   the Meeting lasted no time at all while 6 GB of its audio sat on disk.
/// * **`updated_at`.** The last thing that wrote to the row — the final
///   transcript segment, usually. The only evidence there is when no audio
///   survived.
fn interrupted_end(
    started_at: &str,
    last_touched: Option<String>,
    recovered_bytes: Option<u64>,
) -> String {
    let fallback = || {
        last_touched
            .clone()
            .unwrap_or_else(|| started_at.to_string())
    };
    let Some(bytes) = recovered_bytes else {
        return fallback();
    };
    let Ok(start) = chrono::DateTime::parse_from_rfc3339(started_at) else {
        return fallback();
    };
    // The encoder is CBR, so a file's length is its duration — no decode pass
    // at startup to learn how long a Meeting ran.
    let recorded = chrono::TimeDelta::try_seconds(audio::sink::seconds_from_bytes(bytes) as i64);
    let Some(from_audio) = recorded.map(|delta| start + delta) else {
        return fallback();
    };
    // Whichever is later. Audio usually wins, but a Meeting that transcribed
    // past the last audio to reach disk has evidence of running longer than
    // the audio proves, and discarding that would be the same mistake in
    // reverse.
    match last_touched
        .as_deref()
        .and_then(|touched| chrono::DateTime::parse_from_rfc3339(touched).ok())
    {
        Some(touched) if touched > from_audio => touched.to_rfc3339(),
        _ => from_audio.to_rfc3339(),
    }
}

/// Persists transcript segments as they are produced, and announces them.
async fn write_segments(
    store: Store,
    mirror_wake: Arc<Notify>,
    notifications: broadcast::Sender<ServerNotification>,
    meeting_id: String,
    mut segments: mpsc::Receiver<crate::asr::pipeline::TranscribedSegment>,
) {
    while let Some(segment) = segments.recv().await {
        let id = meeting_id.clone();
        let written = store
            .write(move |connection| {
                meetings::append_segment(
                    connection,
                    &id,
                    segment.channel,
                    segment.start_ms as i64,
                    segment.end_ms as i64,
                    &segment.text,
                )
            })
            .await;

        match written {
            Ok(row) => {
                let _ = notifications.send(ServerNotification::TranscriptSegmentAdded(
                    TranscriptSegmentAddedParams {
                        meeting_id: meeting_id.clone(),
                        segment: row,
                    },
                ));
                mirror_wake.notify_one();
            }
            Err(error) => {
                // Losing a segment is bad; losing the recording because a
                // write failed would be worse.
                warn!(meeting = meeting_id, %error, "could not persist a transcript segment");
            }
        }
    }
    debug!(meeting = meeting_id, "transcript writer finished");
}

fn live_source_factory() -> SourceFactory {
    Arc::new(|| Box::new(audio::live::LiveSource::new()))
}

impl Core {
    /// Creates the Core and its History layout.
    pub fn new() -> Result<Arc<Self>> {
        Self::with_paths_and_models(paths::history_dir(), Settings::path(), paths::models_dir())
    }

    /// Same, against an explicit History folder. Tests use this so they never
    /// depend on process-global paths.
    pub fn with_history_dir(history_dir: std::path::PathBuf) -> Result<Arc<Self>> {
        Self::with_paths(history_dir, Settings::path())
    }

    /// A Core whose Briefing is already acknowledged, with settings scoped
    /// to the History folder.
    ///
    /// For tests about anything *other* than the consent gate. Tests of the
    /// gate itself use `with_history_dir`, which starts unacknowledged like
    /// a real fresh install.
    pub fn with_history_dir_acknowledged(history_dir: std::path::PathBuf) -> Result<Arc<Self>> {
        let settings_path = history_dir.join(".settings-test.json");
        crate::settings::Settings {
            briefing_acknowledged: true,
            ..Default::default()
        }
        .save_to(&settings_path)?;
        Self::with_paths(history_dir, settings_path)
    }

    /// Same, with an explicit settings file. Tests use this so they never
    /// read or write the real machine's acknowledgment state — nor load its
    /// models: this scopes them under the History folder, so a Core built
    /// here finds no model unless the test put one there.
    pub fn with_paths(
        history_dir: std::path::PathBuf,
        settings_path: std::path::PathBuf,
    ) -> Result<Arc<Self>> {
        let models_dir = history_dir.join(paths::DATA_DIR_NAME).join("models");
        Self::with_paths_and_models(history_dir, settings_path, models_dir)
    }

    /// Every path stated outright. Production names the real three; tests
    /// name temporary ones.
    pub fn with_paths_and_models(
        history_dir: std::path::PathBuf,
        settings_path: std::path::PathBuf,
        models_dir: std::path::PathBuf,
    ) -> Result<Arc<Self>> {
        let incomplete_copy = paths::detect_incomplete_copy(&history_dir).then(|| {
            format!(
                "{} holds Mirrors but no machine store — this looks like an incomplete copy, \
                 so transcripts, audio, and Voiceprints may have been left behind. Copy the \
                 whole folder, hidden files included.",
                history_dir.display()
            )
        });
        paths::ensure_history_layout(&history_dir)?;
        if let Some(warning) = &incomplete_copy {
            warn!("{warning}");
        }

        let store = Store::open(
            &history_dir
                .join(paths::DATA_DIR_NAME)
                .join("EverTranscript.db"),
        )?;
        let mirror = MirrorWriter::new(store.clone(), history_dir.clone());

        Ok(Arc::new(Self {
            started_at: Instant::now(),
            history_dir,
            state: Mutex::new(CoreState::Idle),
            incomplete_copy,
            store,
            mirror,
            mirror_wake: Arc::new(Notify::new()),
            diarize_wake: Arc::new(Notify::new()),
            recorder: Mutex::new(None),
            recording: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            source_factory: Mutex::new(live_source_factory()),
            notifications: broadcast::channel(NOTIFICATION_CAPACITY).0,
            transcriber_factory: Mutex::new(None),
            summary_backend_factory: std::sync::Mutex::new(None),
            summarizing: Mutex::new(()),
            fetching: std::sync::Mutex::new(None),
            settings: Mutex::new(Settings::load_from(&settings_path)),
            settings_path,
            models_dir,
            diarization: Mutex::new(None),
        }))
    }

    /// Settings as a Client sees them, including whether the login-item
    /// registration actually matches the setting.
    pub async fn settings(&self) -> SettingsResponse {
        let settings = self.settings.lock().await.clone();
        SettingsResponse {
            briefing_acknowledged: settings.briefing_acknowledged,
            launch_at_login: settings.launch_at_login,
            auto_record: settings.auto_record,
            chinese_script: settings.chinese_script,
            launch_at_login_location: autostart::describe(),
            launch_at_login_registered: autostart::is_enabled(),
            summary_backend: settings.summary_backend,
            summary_strict: settings.summary_strict,
            summary_cloud_warning_accepted: settings.summary_cloud_warning_accepted,
            summary_prompt: settings.summary_prompt,
            // Sent so a Client can show the default and offer reset without
            // keeping its own copy, which would drift the first time this
            // one is edited.
            summary_prompt_default: summary::prompt::DEFAULT_SYSTEM_PROMPT.to_string(),
            check_for_updates: settings.check_for_updates,
        }
    }

    /// Applies a settings change. Only the fields present are touched.
    pub async fn update_settings(&self, change: SettingsSetParams) -> Result<SettingsResponse> {
        {
            let mut settings = self.settings.lock().await;
            if let Some(acknowledged) = change.briefing_acknowledged {
                // One-way on purpose: consent that can be un-given by a
                // Client is not a pre-capture invariant, it is a toggle.
                if acknowledged {
                    settings.briefing_acknowledged = true;
                }
            }
            if let Some(auto_record) = change.auto_record {
                settings.auto_record = auto_record;
            }
            if let Some(script) = change.chinese_script {
                // Takes effect for the next Meeting: the running one read it
                // when it started, and a transcript written two ways would be
                // worse than one written in the script the Operator has since
                // changed their mind about.
                settings.chinese_script = script;
            }
            if let Some(launch_at_login) = change.launch_at_login {
                settings.launch_at_login = launch_at_login;
                // Registration only: a running Core is untouched (story 9c).
                if let Err(error) = autostart::set_enabled(launch_at_login) {
                    warn!(%error, "could not change the login item");
                }
            }
            if let Some(accepted) = change.summary_cloud_warning_accepted {
                // One-way, like the Briefing: a warning that a Client can
                // un-accept is not a gate.
                if accepted {
                    settings.summary_cloud_warning_accepted = true;
                }
            }
            if let Some(backend) = change.summary_backend {
                // The gate lives here rather than in the UI: a Client that
                // forgot to show the warning must not be able to route a
                // transcript to a provider (story 36, ADR-0013).
                if backend != "local" && !settings.summary_cloud_warning_accepted {
                    anyhow::bail!(
                        "choosing a cloud Summary Backend requires accepting the one-time \
                         warning about what leaves this machine"
                    );
                }
                settings.summary_backend = Some(backend);
            }
            if let Some(base_url) = change.summary_base_url {
                settings.summary_base_url = Some(base_url);
            }
            if let Some(check) = change.check_for_updates {
                settings.check_for_updates = check;
            }
            if let Some(strict) = change.summary_strict {
                settings.summary_strict = strict;
            }
            if let Some(prompt) = change.summary_prompt {
                // Empty resets to the default (story 42). Storing the
                // default's text instead would freeze a copy that stops
                // matching the real one the next time it improves.
                settings.summary_prompt = (!prompt.trim().is_empty()).then_some(prompt);
            }
            settings.save_to(&self.settings_path)?;
        }
        Ok(self.settings().await)
    }

    // ------------------------------------------------------------ Watchlist

    /// What Meeting Detection watches here, and what it offers.
    pub async fn watchlist(&self) -> Result<WatchlistResponse> {
        let list = self.store.read(crate::store::watchlist::load).await?;
        Ok(describe_watchlist(&list))
    }

    /// Adds an app. Membership is the per-app switch (ADR-0030), so this is
    /// the whole of "enable an app" — there is no flag to set afterwards.
    pub async fn watchlist_add(&self, params: WatchlistAddParams) -> Result<WatchlistResponse> {
        // A suggested entry carries its own name and kind, so a Client can
        // promote one by id alone rather than restating what the Core knows.
        let suggested = crate::detect::watchlist::suggested_entries()
            .into_iter()
            .find(|entry| entry.id == params.id);
        let entry = crate::detect::watchlist::WatchlistEntry {
            id: params.id.clone(),
            name: params
                .name
                .clone()
                .or_else(|| suggested.as_ref().map(|entry| entry.name.clone()))
                .unwrap_or_else(|| params.id.clone()),
            kind: match params.kind {
                Some(WatchlistKind::BrowserMeetings) => {
                    crate::detect::watchlist::EntryKind::BrowserMeetings
                }
                Some(WatchlistKind::Process) => crate::detect::watchlist::EntryKind::Process,
                None => suggested
                    .as_ref()
                    .map(|entry| entry.kind)
                    .unwrap_or(crate::detect::watchlist::EntryKind::Process),
            },
        };
        self.store
            .write(move |connection| crate::store::watchlist::add(connection, &entry))
            .await?;
        self.watchlist().await
    }

    pub async fn watchlist_remove(&self, id: &str) -> Result<WatchlistResponse> {
        let id = id.to_string();
        self.store
            .write(move |connection| crate::store::watchlist::remove(connection, &id))
            .await?;
        self.watchlist().await
    }

    /// Replaces a Meeting's Notes (ADR-0018).
    pub async fn set_notes(&self, id: &str, notes: &str) -> Result<Meeting> {
        let id = self
            .resolve_meeting(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no Meeting with id {id}"))?;
        let notes = notes.to_string();
        let meeting = self
            .store
            .write(move |connection| crate::store::meetings::set_notes(connection, &id, &notes))
            .await?;
        // The folder follows the database. Notes are the reason an Operator
        // opens the Mirror at all, so a stale one here is worse than a stale
        // transcript.
        self.mirror_wake.notify_one();
        Ok(meeting)
    }

    /// Asks the OS for calendar access (ADR-0036). The calendar poll sees a
    /// grant on its own, so nothing else has to be told.
    ///
    /// On a blocking thread so the runtime's workers stay free. The server
    /// answers it off its loop, so an unanswered dialog, which
    /// `calendar::request` waits on for up to five minutes, holds up no
    /// other request.
    pub async fn request_calendar_access(&self) -> Result<CalendarAccessResponse> {
        let answer = tokio::task::spawn_blocking(crate::detect::calendar::request).await?;
        let granted = answer == crate::detect::calendar::Access::Granted;
        info!(granted, "calendar access was asked for");
        Ok(CalendarAccessResponse { granted })
    }

    /// What this installation holds and may say (stories 46, 47).
    ///
    /// Counted from the record and read from the settings each time rather
    /// than cached: a stale privacy page is a false one, and this is the
    /// surface an evaluator uses to decide.
    pub async fn posture(&self) -> Result<PostureResponse> {
        let settings = self.settings.lock().await.clone();
        let (meetings, speakers, voiceprints) = self
            .store
            .read(|connection| {
                let meetings: i64 =
                    connection.query_row("SELECT COUNT(*) FROM meetings", [], |row| row.get(0))?;
                let speakers: i64 =
                    connection.query_row("SELECT COUNT(*) FROM speakers", [], |row| row.get(0))?;
                let voiceprints: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM speakers WHERE voiceprint IS NOT NULL",
                    [],
                    |row| row.get(0),
                )?;
                Ok((meetings, speakers, voiceprints))
            })
            .await?;

        let models: Vec<String> = crate::models::registry::ALL
            .iter()
            .filter(|entry| entry.local_path(&self.models_dir).exists())
            .map(|entry| entry.display_name.to_string())
            .collect();
        let all_present = crate::models::registry::required()
            .all(|entry| entry.local_path(&self.models_dir).exists());

        let traffic = crate::posture::sanctioned_traffic(
            settings.check_for_updates,
            !all_present,
            settings.summary_backend.as_deref(),
            settings.summary_base_url.as_deref(),
        );
        let currently_silent = crate::posture::currently_silent(&traffic, all_present);

        let claim = |item: &crate::posture::Foreclosed| PostureClaim {
            capability: item.capability.to_string(),
            proof: item.proof.to_string(),
        };

        Ok(PostureResponse {
            history_dir: self.history_dir.display().to_string(),
            meetings,
            speakers,
            voiceprints,
            models,
            calendar_granted: crate::detect::calendar::access()
                == crate::detect::calendar::Access::Granted,
            traffic: traffic
                .into_iter()
                .map(|entry| TrafficEntry {
                    name: entry.name.to_string(),
                    host: entry.host,
                    what_it_sends: entry.what_it_sends.to_string(),
                    enabled: entry.enabled,
                    disableable: entry.disableable,
                })
                .collect(),
            foreclosed: crate::posture::FORECLOSED.iter().map(claim).collect(),
            amended: crate::posture::AMENDED.iter().map(claim).collect(),
            currently_silent,
            source: "https://github.com/EverTranscript/EverTranscript".to_string(),
        })
    }

    // ---- Summary (M4) ----

    /// Builds the Backend the Operator chose, and the local one to fall back
    /// to.
    ///
    /// Returned as a pair on purpose: the fallback is *always* local, and
    /// constructing it here rather than on demand inside the failure path
    /// means there is no branch where a failure could reach for a cloud one.
    fn backends(&self, settings: &crate::settings::Settings) -> Result<ChosenBackends> {
        if let Some(factory) = self
            .summary_backend_factory
            .lock()
            .expect("the summary backend factory mutex is never held across a panic")
            .as_ref()
        {
            return Ok(factory());
        }

        // **Why the local Backend is unavailable, not merely that it is.**
        //
        // These four collapsed into one sentence — "the local Summary model
        // is not available" — which was tolerable when the model was small
        // and Operator-fetched. With Local preselected and a multi-gigabyte
        // fetch running in the background, the likeliest case is a model that
        // is *arriving*, and telling someone it is unavailable while a
        // gigabyte of it sits on their disk is the DECISIONS Q47 mistake: true,
        // and useless.
        let local = || -> Result<Box<dyn summary::Backend + 'static>, String> {
            let entry = crate::models::registry::SUMMARY_DEFAULT;
            let model = self.models_dir.join(entry.filename);

            let downloader = models::Downloader::new(self.models_dir.clone())
                .map_err(|error| format!("the models folder is unreadable: {error}"))?;
            match downloader.status(&entry) {
                models::ModelStatus::Ready { .. } => {}
                models::ModelStatus::Partial { bytes_on_disk } => {
                    return Err(format!(
                        "the Summary model is still downloading — {} of {} MB so far",
                        bytes_on_disk / 1_048_576,
                        entry.integrity.size_bytes / 1_048_576
                    ));
                }
                models::ModelStatus::Corrupted { reason } => {
                    return Err(format!(
                        "the Summary model on disk is damaged and will be fetched again: {reason}"
                    ));
                }
                models::ModelStatus::Missing => {
                    return Err(format!(
                        "the Summary model has not been downloaded — {} MB",
                        entry.integrity.size_bytes / 1_048_576
                    ));
                }
            }

            let binary = summarizer_binary()
                .ok_or_else(|| "the Summary engine is missing from this install".to_string())?;
            let driving = entry
                .driving
                .as_ref()
                .map(summary::sidecar::Driving::from_entry);
            summary::sidecar::SidecarBackend::spawn_driven(
                &binary,
                &model.to_string_lossy(),
                driving,
            )
            .map(|backend| Box::new(backend) as Box<dyn summary::Backend + 'static>)
            // The model is on disk and verified, so a failure here is the
            // machine refusing to hold it — a different problem from a
            // missing file, with a different answer for the Operator.
            .map_err(|error| {
                format!(
                    "the Summary model is present but would not load on this machine, \
                     which usually means it does not fit: {error}"
                )
            })
        };

        let choice = settings
            .summary_backend
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("no Summary Backend has been chosen"))?;

        if choice == "local" {
            let backend = local().map_err(|reason| anyhow::anyhow!("{reason}"))?;
            return Ok((backend, None));
        }

        let (display, base_url, model) = match summary::cloud::preset(choice) {
            Some(preset) => (
                preset.display_name.to_string(),
                preset.base_url.to_string(),
                preset.default_model.to_string(),
            ),
            // A custom endpoint. Its terms are not ours to characterise
            // (ADR-0010), and it is offered anyway.
            None => (
                choice.to_string(),
                settings
                    .summary_base_url
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("no base URL for {choice}"))?,
                "default".to_string(),
            ),
        };

        let key = summary::credentials::get(choice).ok().flatten();
        let chosen = summary::cloud::CloudBackend::new(&display, &base_url, &model, key)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        // A cloud choice keeps local as its Fallback when local is available;
        // when it is not, the reason is logged rather than lost, and the cloud
        // Backend still runs.
        let fallback = match local() {
            Ok(backend) => Some(backend),
            Err(reason) => {
                debug!(reason, "no local Fallback is available");
                None
            }
        };
        Ok((Box::new(chosen), fallback))
    }

    /// Generates a Summary for a finished Meeting.
    pub async fn summarize_meeting(&self, meeting_id: &str) -> Result<String> {
        // Before the Meeting is read, so a run that waited reads it as it is
        // now rather than as it was before the wait.
        let _one_at_a_time = self.summarizing.lock().await;
        let Some((meeting, segments)) = self.get_meeting(meeting_id).await? else {
            anyhow::bail!("no Meeting with id {meeting_id}");
        };
        if segments.is_empty() {
            anyhow::bail!("this Meeting has no transcript to summarize");
        }

        let settings = self.settings.lock().await.clone();
        let knob = summary::knob::Knob {
            choice: settings.summary_backend.as_deref().map(|choice| {
                if choice == "local" {
                    summary::knob::Choice::Local
                } else {
                    summary::knob::Choice::Cloud {
                        provider: choice.to_string(),
                    }
                }
            }),
            strict: settings.summary_strict,
            cloud_warning_accepted: settings.summary_cloud_warning_accepted,
        };
        if !knob.is_configured() {
            anyhow::bail!(
                "no Summary Backend has been chosen — pick Local or Cloud first (ADR-0013)"
            );
        }

        // **Escaped, like Notes.** Under the plain framing a stray
        // `<|im_end|>` in the Operator's prompt was characters; under a chat
        // template it ends the system turn. The reasoning the Notes armor
        // already carries applies here word for word — the Operator is
        // trusted, and text they pasted from somewhere else is not
        // necessarily.
        let system = summary::prompt::escape_control_markers(
            &settings
                .summary_prompt
                .clone()
                .unwrap_or_else(|| summary::prompt::DEFAULT_SYSTEM_PROMPT.to_string()),
        );
        let names = self
            .store
            .read(crate::store::speakers::list)
            .await?
            .into_iter()
            .filter_map(|speaker| {
                let label = match (speaker.display_name, speaker.is_operator) {
                    (Some(name), _) => name,
                    (None, true) => "You".to_string(),
                    (None, false) => return None,
                };
                Some((speaker.id, label))
            })
            .collect::<std::collections::BTreeMap<_, _>>();

        let (mut chosen, mut fallback) = self.backends(&settings)?;
        let notes = meeting.notes.clone();
        let cancel = summary::Cancel::new();

        // Generation is minutes of CPU. Off the async runtime, like
        // Diarization, so a Summary cannot stall the Core's answers to
        // Clients — or, worse, a recording.
        let outcome = tokio::task::spawn_blocking(move || -> Result<SummaryRun> {
            let lookup = |id: &str| names.get(id).cloned();
            let material = summary::generate::Material {
                segments: &segments,
                speaker_names: &lookup,
                notes: notes.as_deref(),
            };
            let transcript = summary::generate::render_transcript(&material);
            // To the registered model's own budget rather than a constant
            // sized for the model before it.
            let single_pass = crate::models::registry::SUMMARY_DEFAULT
                .driving
                .as_ref()
                .map(|driving| driving.single_pass_tokens)
                .unwrap_or(summary::generate::SINGLE_PASS_TOKENS);
            let chunks = summary::generate::chunk_to(&transcript, single_pass);
            let request_for = |piece: &str| summary::Request {
                system: system.clone(),
                user: summary::prompt::build_user_message(notes.as_deref(), piece),
            };

            // **The first chunk chooses the Backend for the whole run.** The
            // Knob decides once, here; everything after it runs on whoever
            // answered. A per-chunk Knob would let a mid-meeting hiccup stitch
            // one record out of two models under a label naming one of them,
            // which is a worse outcome than the Summary being local throughout.
            let fallback_ref: Option<&mut dyn summary::Backend> = match fallback.as_mut() {
                Some(backend) => Some(backend.as_mut()),
                None => None,
            };
            let first = summary::knob::run(
                &knob,
                chosen.as_mut(),
                fallback_ref,
                &request_for(&chunks[0]),
                &cancel,
            )
            .map_err(|error| anyhow::anyhow!("{error}"))?;
            let used = first.used.clone();
            let fell_back_from = first.fell_back_from.clone();

            // Whoever served chunk one serves the rest. `fell_back_from` is
            // the only thing that can tell us which that was, and it is set by
            // the Knob rather than inferred here.
            let winner: &mut dyn summary::Backend = if fell_back_from.is_some() {
                fallback
                    .as_mut()
                    .expect("a Fallback happened, so there was a fallback Backend")
                    .as_mut()
            } else {
                chosen.as_mut()
            };

            // Map. A chunk that fails is skipped rather than fatal: five parts
            // of six is a usable record of the meeting and none is not.
            // Cancellation is the Operator, not a bad chunk — it stops.
            //
            // **A chunk that comes back injected counts as a chunk that
            // failed.** `prompt::verify` refuses output that is not a summary
            // of the piece it was given, which is measured to happen: the
            // registered model obeys instructions written into a transcript.
            // Treating it as a failure rather than a fatal error is the same
            // judgement as above — one poisoned chunk should not cost the
            // Operator the other five — and the count reaches the record
            // through `gaps`, so the loss is visible rather than silent.
            //
            // **The reason travels with the refusal.** The daemon writes no
            // log the Operator can read, so a `warn!` here is a reason nobody
            // will ever see; a Summary that lost half a meeting was telling
            // them the fraction and not the cause.
            fn kept(text: &str, piece: &str) -> Result<(String, usize), String> {
                let part = summary::prompt::scrub(text);
                // Before verifying, not after: an item credited to the
                // unnamed-Speaker placeholder names nobody, and refusing the
                // chunk over it costs the Operator the rest of the Summary.
                let (part, dropped) = summary::prompt::drop_placeholder_items(&part);
                match summary::prompt::verify(&part, piece) {
                    Ok(()) => Ok((part, dropped)),
                    Err(why) => {
                        tracing::warn!(
                            %why,
                            "a Summary was refused: it is not a summary of this meeting"
                        );
                        Err(why.to_string())
                    }
                }
            }

            let mut parts = Vec::with_capacity(chunks.len());
            let mut failed = 0usize;
            // Items taken out because they credited the unnamed-Speaker
            // placeholder. Counted so the record can say so: a table quietly
            // one row shorter is the kind of edit this product does not make.
            let mut dropped_items = 0usize;
            // The first one only. A list of every refusal would be a wall
            // of text in what is one line of a record, and the first is the
            // one an Operator can still find the transcript for.
            let mut refusal = None;
            let mut note = |why: String| {
                failed += 1;
                refusal.get_or_insert(why);
            };
            match kept(&first.text, &chunks[0]) {
                Ok((part, dropped)) => {
                    dropped_items += dropped;
                    parts.push(part);
                }
                Err(why) => note(why),
            }
            for piece in &chunks[1..] {
                if cancel.is_cancelled() {
                    anyhow::bail!("{}", summary::BackendError::Cancelled);
                }
                match winner.generate(&request_for(piece), &cancel) {
                    Ok(text) => match kept(&text, piece) {
                        Ok((part, dropped)) => {
                            dropped_items += dropped;
                            parts.push(part);
                        }
                        Err(why) => note(why),
                    },
                    Err(summary::BackendError::Cancelled) => {
                        anyhow::bail!("{}", summary::BackendError::Cancelled)
                    }
                    // A Backend that failed outright is a lost chunk too,
                    // and its own message says more than "could not".
                    Err(error) => note(error.to_string()),
                }
            }

            // Nothing survived. A Meeting with no Summary is a gap the
            // Operator can fill by regenerating; a Meeting whose Summary is
            // whatever the transcript told the model to write is a false
            // record, and ADR-0009 will not let them edit it out.
            if parts.is_empty() {
                // **With the reason, when there is one.** This sentence is
                // what an Operator receives when a meeting gets no Summary at
                // all, which is the case that most needs explaining, and on
                // its own it sends them to a log the daemon does not write.
                let why = match &refusal {
                    Some(why) => format!(" — {why}"),
                    None => String::new(),
                };
                anyhow::bail!(
                    "{}",
                    summary::BackendError::Malformed(format!(
                        "the Backend returned nothing that was a summary of \
                         this meeting{why}"
                    ))
                );
            }

            let text = if chunks.len() == 1 {
                parts.remove(0)
            } else {
                // Reduce: one more pass over the partial summaries. A failed
                // reduce is not a failed run — the parts are still a record of
                // the meeting, and discarding them because the last call timed
                // out would waste every call before it.
                if cancel.is_cancelled() {
                    anyhow::bail!("{}", summary::BackendError::Cancelled);
                }
                let combined = parts.join("\n\n---\n\n");
                let reduce = request_for(&summary::prompt::reduce_message(&combined));
                match winner.generate(&reduce, &cancel) {
                    // Checked against the whole transcript rather than the
                    // partial summaries it was handed, because that is what
                    // its action items are claims about.
                    Ok(text) => {
                        let reduced = summary::prompt::scrub(&text);
                        // The same order as the map stage, because the reduce
                        // writes its own table: an item crediting the
                        // placeholder is taken out before the check rather
                        // than refused by it, or a fresh one here would throw
                        // away the whole reduce.
                        let (reduced, dropped) = summary::prompt::drop_placeholder_items(&reduced);
                        match summary::prompt::verify(&reduced, &transcript) {
                            Ok(()) => {
                                // Only on the branch whose text survives —
                                // the fallback discards these rows anyway.
                                dropped_items += dropped;
                                reduced
                            }
                            // The parts are already verified, so falling back
                            // to them loses polish rather than truth.
                            Err(why) => {
                                tracing::warn!(%why, "the reduce pass was refused");
                                combined
                            }
                        }
                    }
                    Err(summary::BackendError::Cancelled) => {
                        anyhow::bail!("{}", summary::BackendError::Cancelled)
                    }
                    Err(_) => combined,
                }
            };

            Ok(SummaryRun {
                text,
                used,
                fell_back_from,
                chunks: chunks.len(),
                failed_chunks: failed,
                refusal,
                dropped_items,
            })
        })
        .await??;

        // Already scrubbed per part inside the run.
        let markdown = outcome.text.clone();
        let used = outcome.used.label();
        // **What the Summary lost, in the record rather than only the log.**
        // A Summary assembled from five chunks of six is a different thing
        // from a complete one, and the Operator cannot read the Core's log.
        let mut gaps = (outcome.failed_chunks > 0).then(|| {
            tracing::warn!(
                failed = outcome.failed_chunks,
                of = outcome.chunks,
                "some chunks of this Meeting could not be summarized"
            );
            let mut note = format!(
                "{} of {} parts of this meeting could not be summarized, \
                 so this Summary does not cover all of it.",
                outcome.failed_chunks, outcome.chunks
            );
            if let Some(why) = &outcome.refusal {
                note.push_str(&format!(" The first one failed because {why}."));
            }
            note
        });
        // **A table one row shorter is a changed record, so it is said out
        // loud.** These were items the model credited to an unnamed Speaker,
        // which names nobody and cannot be checked against a person.
        if outcome.dropped_items > 0 {
            tracing::warn!(
                dropped = outcome.dropped_items,
                "action items credited to no named Speaker were left out"
            );
            let plural = if outcome.dropped_items == 1 {
                "item was"
            } else {
                "items were"
            };
            let note = format!(
                "{} action {plural} left out for crediting an unnamed speaker \
                 rather than a person.",
                outcome.dropped_items
            );
            gaps = Some(match gaps {
                Some(existing) => format!("{existing} {note}"),
                None => note,
            });
        }
        if let Some(from) = &outcome.fell_back_from {
            // Never silent: an Operator who chose Cloud and received local
            // quality is owed the reason.
            tracing::warn!(from = %from, to = %used, "the Summary Backend fell back");
        }

        // The Title Chain's third slot (ADR-0030 as amended by ADR-0036). The
        // heading is only ever a *suggestion*: the store applies it where the
        // name is still absent, so a person's word and the calendar's both
        // outrank it without this call site having to know the rule.
        let suggested_title = summary::prompt::title_from(&markdown);

        // **The Meeting's own id, not the one that was typed.** `get_meeting`
        // accepts the short form the Mirror's filename carries, and this used
        // to write back under whatever the caller passed — so a Summary asked
        // for by short id was generated in full, cost every token it cost, and
        // then hit an `UPDATE ... WHERE id` that matched nothing. The Operator
        // got `no Meeting with id`, naming an id that had just resolved.
        let id = meeting.id.clone();
        let stored = markdown.clone();
        let label = used.clone();
        self.store
            .write(move |connection| {
                crate::store::meetings::set_summary(
                    connection,
                    &id,
                    &stored,
                    &label,
                    suggested_title.as_deref(),
                    gaps.as_deref(),
                )
            })
            .await?;
        // The Mirror's filename follows the title, so a Meeting the Summary
        // just named needs its file renamed before anyone looks.
        self.mirror.rebuild_pending().await?;
        self.mirror_wake.notify_one();
        Ok(markdown)
    }

    /// The Summary destinations this build offers.
    pub async fn summary_backends(&self) -> SummaryBackendsResponse {
        let settings = self.settings.lock().await.clone();
        let mut options = vec![SummaryBackendOption {
            id: "local".into(),
            display_name: "Local (recommended)".into(),
            leaves_the_machine: false,
            has_key: false,
            data_handling: None,
        }];
        for preset in summary::cloud::PRESETS {
            options.push(SummaryBackendOption {
                id: preset.id.into(),
                display_name: preset.display_name.into(),
                leaves_the_machine: !summary::cloud::is_loopback(preset.base_url),
                // Whether a key is stored — never the key.
                has_key: summary::credentials::exists(preset.id),
                data_handling: preset
                    .data_handling
                    .as_ref()
                    .map(|handling| SummaryDataHandling {
                        trains_on_inputs: handling.trains_on_inputs,
                        retention: handling.retention.to_string(),
                        zero_retention_available: handling.zero_retention_available,
                        verified_on: handling.verified_on.to_string(),
                    }),
            });
        }
        SummaryBackendsResponse {
            options,
            chosen: settings.summary_backend,
            strict: settings.summary_strict,
            cloud_warning_accepted: settings.summary_cloud_warning_accepted,
            custom_endpoint_label: summary::cloud::CUSTOM_ENDPOINT_LABEL.to_string(),
        }
    }

    /// Stores or clears an API key. It is never read back to a Client.
    pub async fn set_summary_key(
        &self,
        provider: &str,
        key: Option<&str>,
    ) -> Result<SummaryBackendsResponse> {
        match key {
            Some(key) if !key.trim().is_empty() => {
                summary::credentials::set(provider, key.trim())
                    .map_err(|error| anyhow::anyhow!("{error}"))?;
            }
            // Clearing is a first-class act, never a side effect of
            // switching the Knob: an Operator may be going local for one
            // meeting, and deleting their key would punish that.
            _ => {
                summary::credentials::delete(provider)
                    .map_err(|error| anyhow::anyhow!("{error}"))?;
            }
        }
        Ok(self.summary_backends().await)
    }

    // ---- Speakers and the Voice Registry (M3) ----

    /// Every Speaker the app holds (story 30).
    pub async fn speakers(&self) -> Result<SpeakerListResponse> {
        let speakers = self
            .store
            .read(|connection| {
                let rows = crate::store::speakers::list(connection)?;
                rows.into_iter()
                    .map(|row| speaker_to_wire(connection, row))
                    .collect::<Result<Vec<_>>>()
            })
            .await?;
        Ok(SpeakerListResponse { speakers })
    }

    /// One Speaker, plus the names the calendar knew for Meetings they were
    /// in.
    ///
    /// Suggestions, never attributions. ADR-0036 stores attendees precisely
    /// so this can offer them, and M2's schema comment already says why they
    /// are not applied: an invitation is evidence about who was invited, and
    /// turning it into who spoke would be inventing attribution.
    pub async fn speaker(&self, id: &str) -> Result<SpeakerDetailResponse> {
        let id = self
            .resolve_speaker(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no Speaker with id {id}"))?;
        self.store
            .read(move |connection| {
                let row = crate::store::speakers::get(connection, &id)?
                    .ok_or_else(|| anyhow::anyhow!("no Speaker with id {id}"))?;
                let speaker = speaker_to_wire(connection, row)?;
                let name_suggestions = crate::store::speakers::name_suggestions(connection, &id)?;
                let meetings = crate::store::speakers::meetings_heard_in(connection, &id)?
                    .into_iter()
                    .map(|heard| SpeakerMeeting {
                        id: heard.meeting_id,
                        started_at: heard.started_at,
                        title: heard.title,
                        detected_app: heard.app,
                    })
                    .collect();
                Ok(SpeakerDetailResponse {
                    speaker,
                    meetings,
                    name_suggestions,
                })
            })
            .await
    }

    /// Names a Speaker, which also confirms its Voiceprint.
    ///
    /// When the name already belongs to another Speaker this does **not**
    /// rename: it answers with what a join would merge and leaves both rows
    /// alone. Called again with `join` set, it folds this Speaker into that
    /// one instead, which is what keeps "naming labels every past
    /// appearance" true once a voice can come back as a new pseudonym
    /// (ADR-0037).
    pub async fn speaker_rename(
        &self,
        id: &str,
        display_name: &str,
        join: bool,
    ) -> Result<SpeakerResponse> {
        let id = self
            .resolve_speaker(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no Speaker with id {id}"))?;
        let display_name = display_name.to_string();
        let outcome = self
            .store
            .write(move |connection| {
                use crate::store::speakers;

                let holder = speakers::by_name(connection, &display_name)?
                    .filter(|existing| existing.id != id);

                match (holder, join) {
                    // The ordinary rename: nobody else has the name.
                    (None, _) => {
                        let row = speakers::rename(connection, &id, &display_name)?;
                        Ok((speaker_to_wire(connection, row)?, None))
                    }
                    // Taken, and the Operator has said to merge.
                    (Some(existing), true) => {
                        let row = speakers::join(connection, &id, &existing.id)?;
                        // Naming is still confirmation: the surviving row
                        // carries the name the Operator just re-asserted.
                        let row = speakers::rename(connection, &row.id, &display_name)?;
                        Ok((speaker_to_wire(connection, row)?, None))
                    }
                    // Taken, and nobody has been asked yet.
                    (Some(existing), false) => {
                        let preview = SpeakerJoinPreview {
                            into_meetings: speakers::appearances(connection, &existing.id)?
                                .meetings,
                            from_meetings: speakers::appearances(connection, &id)?.meetings,
                            into: speaker_to_wire(connection, existing)?,
                        };
                        let unchanged = speakers::get(connection, &id)?
                            .ok_or_else(|| anyhow::anyhow!("no Speaker with id {id}"))?;
                        Ok((speaker_to_wire(connection, unchanged)?, Some(preview)))
                    }
                }
            })
            .await?;

        let (speaker, join_required) = outcome;
        if join_required.is_none() {
            let _ =
                self.notifications
                    .send(ServerNotification::SpeakerChanged(SpeakerChangedParams {
                        speaker: speaker.clone(),
                    }));
            self.mirror_wake.notify_one();
        }
        Ok(SpeakerResponse {
            speaker,
            join_required,
        })
    }

    /// Deletes a Speaker's Voiceprint (story 31). The record is untouched.
    pub async fn speaker_delete_voiceprint(&self, id: &str) -> Result<SpeakerResponse> {
        // A Voiceprint deletion, so the same care as `delete_meeting`: an
        // ambiguous short id is refused rather than resolved to a guess.
        let id = self
            .resolve_speaker(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no Speaker with id {id}"))?;
        let speaker = self
            .store
            .write(move |connection| {
                crate::store::speakers::delete_voiceprint(connection, &id)?;
                let row = crate::store::speakers::get(connection, &id)?
                    .ok_or_else(|| anyhow::anyhow!("no Speaker with id {id}"))?;
                speaker_to_wire(connection, row)
            })
            .await?;
        let _ = self
            .notifications
            .send(ServerNotification::SpeakerChanged(SpeakerChangedParams {
                speaker: speaker.clone(),
            }));
        Ok(SpeakerResponse {
            speaker,
            join_required: None,
        })
    }

    /// A few seconds of a Speaker's voice, cut from the recording their
    /// Voiceprint was taken from.
    ///
    /// The other half of the Registry's legibility: a row can name the
    /// Meeting a voice came from, and now it can play it. `None` rather
    /// than an error when there is nothing to play — a Speaker minted
    /// before samples were kept, or one whose recording has been deleted —
    /// because both are ordinary states of a Registry, not faults.
    pub async fn speaker_sample(&self, id: &str) -> Result<SpeakerSampleResponse> {
        let id = self
            .resolve_speaker(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no Speaker with id {id}"))?;
        let source = self
            .store
            .read(move |connection| {
                let Some(source) = crate::store::speakers::sample_source(connection, &id)? else {
                    return Ok(None);
                };
                let audio_path = crate::store::meetings::get(connection, &source.meeting_id)?
                    .and_then(|meeting| meeting.audio_path);
                Ok(audio_path.map(|path| (source, path)))
            })
            .await?;
        let Some((source, audio_path)) = source else {
            return Ok(SpeakerSampleResponse { sample: None });
        };
        let path = self.history_dir.join(audio_path);
        if !path.exists() {
            return Ok(SpeakerSampleResponse { sample: None });
        }

        let sample = source.sample;
        let clip = tokio::task::spawn_blocking(move || {
            audio::sample::cut(
                &path,
                sample.channel,
                sample.start_ms.max(0) as u64,
                sample.end_ms.max(0) as u64,
            )
        })
        .await??;
        use base64::Engine;
        Ok(SpeakerSampleResponse {
            sample: Some(SpeakerSampleClip {
                audio_base64: base64::engine::general_purpose::STANDARD.encode(&clip.bytes),
                mime_type: clip.mime_type.to_string(),
                meeting_id: source.meeting_id,
                channel: sample.channel,
                start_ms: sample.start_ms,
                end_ms: sample.end_ms,
            }),
        })
    }

    /// Re-assigns a segment to a different Speaker (story 29b).
    pub async fn reassign_segment(
        &self,
        segment_id: &str,
        speaker_id: &str,
    ) -> Result<TranscriptReassignResponse> {
        let segment_id = segment_id.to_string();
        let speaker_id = speaker_id.to_string();
        let (meeting_id, segment) = self
            .store
            .write(move |connection| {
                crate::store::speakers::correct_attribution(connection, &segment_id, &speaker_id)?;
                let meeting_id: String = connection.query_row(
                    "SELECT meeting_id FROM transcript_segments WHERE id = ?1",
                    rusqlite::params![segment_id],
                    |row| row.get(0),
                )?;
                let segment = crate::store::meetings::segments(connection, &meeting_id)?
                    .into_iter()
                    .find(|segment| segment.id == segment_id)
                    .ok_or_else(|| anyhow::anyhow!("the segment vanished after correction"))?;
                Ok((meeting_id, segment))
            })
            .await?;
        let _ = meeting_id;
        self.mirror_wake.notify_one();
        Ok(TranscriptReassignResponse { segment })
    }

    /// Diarizes a finished Meeting, end to end.
    ///
    /// Everything M3 built meets here: decode the kept audio, run the two
    /// models, resolve clusters to persistent Speakers, and map the result
    /// onto a Transcript that already exists. Nothing on this path can cost
    /// the recording — a missing model, a corrupt file, or a panicking
    /// runtime all leave the Meeting exactly as it was, unattributed.
    pub async fn diarize_meeting(
        &self,
        meeting_id: &str,
        priority: crate::store::diarize_queue::Priority,
    ) -> Result<DiarizeOutcome> {
        use crate::diarize;

        // Before anything is read or claimed. The same question is asked
        // again on every progress span below, because a recording that starts
        // during an inference pass has to stop it too — a check only at the
        // door leaves the backlog holding the machine for the length of a
        // Meeting.
        if yields_to_recording(priority, self.is_recording().await) {
            return Ok(DiarizeOutcome::Paused);
        }

        let Some(meeting) = self.get_meeting(meeting_id).await?.map(|(m, _)| m) else {
            anyhow::bail!("no Meeting with id {meeting_id}");
        };
        let Some(audio_path) = meeting.audio_path.clone() else {
            // A Meeting whose audio was never written, or was deleted. Not
            // an error: there is simply nothing to listen to.
            return Ok(DiarizeOutcome::Skipped);
        };
        // Stored relative to the History folder so the record stays portable
        // (ADR-0035) — resolving it is the caller's job, not the row's.
        let audio_path = self.history_dir.join(audio_path);
        if !audio_path.exists() {
            tracing::info!(path = %audio_path.display(), "the Meeting's audio is gone; nothing to diarize");
            return Ok(DiarizeOutcome::Skipped);
        }

        let segmentation = self.models_dir.join("diarize-segmentation.onnx");
        let embedding = self.models_dir.join("diarize-embedding.onnx");
        if !segmentation.exists() || !embedding.exists() {
            tracing::info!(
                "diarization models are not downloaded; leaving the Meeting unattributed"
            );
            return Ok(DiarizeOutcome::Skipped);
        }

        // Claimed before the job entry is written, not inside the spawned
        // task. M3 had it the other way round, and a refused second run
        // therefore overwrote the running job's entry and then cleared it on
        // the way out — the running job became invisible to `diarize/status`
        // and uncancellable. Refusing before anything is written is the
        // whole fix.
        let slot =
            diarize::runner::Slot::claim(meeting_id).map_err(|busy| anyhow::anyhow!("{busy}"))?;

        // What this Meeting already says about its named voices, read before
        // the run and embedded during it. Only a bulk re-run's own Meeting
        // has one: `is_bulk_work` is false on every History in the field,
        // because the re-run tables are not in `MIGRATIONS`.
        //
        // Read here rather than on the writer because `plan` is a read and
        // the embedding that follows is minutes of model time; the copy the
        // writer trusts is the one `reseed::commit` re-reads inside the
        // transaction, against which this one is only a proposal.
        let wanted = meeting_id.to_string();
        let plan = self
            .store
            .read(move |connection| {
                if !crate::store::rerun::is_bulk_work(connection, &wanted)? {
                    return Ok(None);
                }
                diarize::reseed::plan(connection, &wanted)
            })
            .await?;

        // Evidence from a previous model or front end, to be rebuilt from
        // its kept audio before this run reads seeds — otherwise every
        // Speaker History knows would be a stranger to it (ADR-0035,
        // DECISIONS Q115). Read here, re-embedded beside the Meeting below,
        // adopted in the same transaction as this run's own evidence.
        let stale = self
            .store
            .read(|connection| {
                crate::store::speakers::stale_exemplars(
                    connection,
                    diarize::live::EMBEDDING_MODEL,
                    diarize::live::EMBEDDING_MODEL_VERSION,
                )
            })
            .await?;
        let history_dir = self.history_dir.clone();

        let cancel = diarize::Cancel::new();
        {
            // Registration and the check that this work is still wanted are
            // one step, under the lock a stop has to take to reach a running
            // job. Apart, they are a race the worker loses: it reads the head
            // of the queue, a bulk stop removes that row and counts the
            // Meeting abandoned, and the run starts anyway and commits new
            // evidence after the stop. Whichever side takes the lock first
            // now wins cleanly — either the job is registered and the stop
            // finds its handle, or the row is gone and this returns before
            // anything is claimed.
            let mut running = self.diarization.lock().await;
            let owed = meeting_id.to_string();
            if !self
                .store
                .read(move |connection| crate::store::diarize_queue::holds(connection, &owed))
                .await?
            {
                return Ok(DiarizeOutcome::Cancelled);
            }
            *running = Some(DiarizeJob {
                meeting_id: meeting_id.to_string(),
                cancel: cancel.clone(),
                done_ms: 0,
                total_ms: 0,
            });
        }
        let notifications = self.notifications.clone();
        let id_for_progress = meeting_id.to_string();
        // Read on the blocking thread, which is why it is an atomic and not
        // the `recorder` mutex: a recording that starts mid-pass stops this
        // run cooperatively, through the same token `diarize/cancel` uses, so
        // there is one way to stop.
        let stands_down = yields_to_recording(priority, true).then(|| Arc::clone(&self.recording));
        // The same eligibility, kept for after the inference task has dropped
        // its copy: a recording can start while the write is still queued.
        let stands_down_after = stands_down.clone();
        let stop_in_run = cancel.clone();
        // Both of these outlive the blocking task on purpose. The token is the
        // only thing that can say a stop was asked for after `diarize`
        // produced its answer, and the flag is the only thing that can say a
        // recording is why — see `finish_run`.
        let stop = cancel.clone();
        let stood_down = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stood_down_in_run = Arc::clone(&stood_down);

        // The models are CPU-bound C++; keeping them off the async runtime is
        // what stops a long Meeting from stalling every Client request.
        let outcome = tokio::task::spawn_blocking(move || -> RunResult {
            // Moved in so it is released when the blocking task ends,
            // including by a panic out of the ONNX runtime: a slot that
            // stayed claimed would turn one bad Meeting into a permanently
            // broken feature.
            let _slot = slot;
            // Decoding and the stale rebuild below both run before the first
            // progress tick, and both are expensive — a rebuild re-reads and
            // re-embeds a window of kept audio per stale exemplar. A recording
            // that began while this run was waiting for its slot is noticed
            // here rather than after them.
            if stand_down_for_recording(stands_down.as_ref(), &stop_in_run, &stood_down_in_run) {
                return Ok(Ran {
                    result: Err(diarize::DiarizeError::Cancelled),
                    rebuilt: Vec::new(),
                    prepared: None,
                });
            }
            let mut decoded = diarize::runner::decode(&audio_path)?;
            // The far end comes back through the speakers into the
            // microphone, and diarization heard it there as strangers: on
            // the first real History this product kept, nearly every mic
            // voice that was not the Operator coincided with far-end speech.
            // Cancelled here on the way to the models, exactly as the
            // transcription path does — the kept audio stays raw (ADR-0029
            // as amended), so a filter that is wrong costs one run, which
            // can be repeated, and never the record.
            audio::aec::EchoCanceller::new(diarize::fbank::SAMPLE_RATE)
                .process(&mut decoded.mic, &decoded.system);
            let mut diarizer = diarize::live::LiveDiarizer::load(&segmentation, &embedding)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            if stand_down_for_recording(stands_down.as_ref(), &stop_in_run, &stood_down_in_run) {
                return Ok(Ran {
                    result: Err(diarize::DiarizeError::Cancelled),
                    rebuilt: Vec::new(),
                    prepared: None,
                });
            }
            let rebuilt = diarize::runner::rebuild(&stale, &history_dir, &mut |samples| {
                diarizer.embedder().embed(samples)
            });

            // The same embedder the run is about to use, so the bounded
            // vectors and this run's clusters are in one space and the
            // resolve below can compare them. Before the pass rather than
            // after it, because the pass is the long part and a stop during
            // it should not have this still to do.
            let prepared = plan.map(|plan| {
                diarize::reseed::embed_ranges(
                    &plan,
                    &history_dir,
                    &mut |path, channel, start_ms, end_ms| {
                        audio::sample::read(path, channel, start_ms, end_ms)
                    },
                    &mut |samples| diarizer.embedder().embed(samples),
                )
                .map(|vectors| Prepared { plan, vectors })
                .map_err(|error| error.to_string())
            });

            let mut last_percent = u64::MAX;
            let result = diarize::runner::run_guarded(
                &mut diarizer,
                decoded.audio(),
                &mut |progress| {
                    stand_down_for_recording(
                        stands_down.as_ref(),
                        &stop_in_run,
                        &stood_down_in_run,
                    );
                    // Throttled to whole percent: a notification per span
                    // would flood every attached Client with numbers nobody
                    // reads.
                    let percent = (progress.fraction() * 100.0) as u64;
                    if percent != last_percent {
                        last_percent = percent;
                        let _ = notifications.send(ServerNotification::DiarizeProgress(
                            evertranscript_protocol::DiarizeProgressParams {
                                meeting_id: id_for_progress.clone(),
                                state: DiarizeState::Running,
                                done_ms: progress.done_ms as i64,
                                total_ms: progress.total_ms as i64,
                            },
                        ));
                    }
                },
                &cancel,
            );
            Ok(Ran {
                result,
                rebuilt,
                prepared,
            })
        })
        .await;
        // Not `?`. A `JoinError` is the task panicking outside `run_guarded`'s
        // guard — in `decode` or the rebuild — and returning here would leave
        // `self.diarization` holding a job that no longer exists, so
        // `diarize/status` would report a run for ever and `diarize/cancel`
        // would cancel a thread that is gone.
        let outcome = outcome.unwrap_or_else(|join| {
            Err(anyhow::anyhow!(
                "the diarization task did not finish: {join}"
            ))
        });

        // The job stays registered across completion and is cleared after it,
        // whatever the answer. Clearing it first put `diarize/cancel` out of
        // reach for the whole of the persistence step.
        let finished = self
            .finish_run(meeting_id, outcome, &stop, &stood_down, stands_down_after)
            .await;
        *self.diarization.lock().await = None;
        finished
    }

    /// Turns a finished run into an outcome, and writes its evidence — or
    /// writes nothing and says the Meeting is still owed.
    ///
    /// **The last place a stop can be honoured, and where that is decided.**
    /// `LiveDiarizer::observe` polls the token at window starts only, and
    /// `diarize` returns `Ok` after its final progress tick, so a stop asked
    /// for during the last window, during clustering, or during the tick
    /// itself arrives *after* a successful result exists. Taking that result on
    /// trust is what made "an interrupted run writes nothing" untrue: the
    /// transaction adopts rebuilt Voiceprints, mints Speakers, moves
    /// attributions and marks the Meeting diarized.
    ///
    /// The check therefore lives **inside the writer closure**, not before the
    /// call. [`crate::store::Store::write`] queues a closure onto a single
    /// writer thread and waits for it, so a check on this side of that
    /// `.await` is a check before an unbounded wait: a Meeting can start
    /// recording while the closure is still in the queue. The closure runs on
    /// the writer with the connection in hand and no transaction open yet,
    /// which is the real commit boundary. Nothing here interrupts a commit
    /// already under way, and nothing needs to — the choice is made before one
    /// begins.
    ///
    /// `stood_down` is why the run stopped, recorded when the token was set
    /// rather than inferred now: a recording that has already ended by the time
    /// this runs must still leave the Meeting owed. `stands_down` is the
    /// eligibility, kept live because a recording that starts *during* this
    /// step has to be noticed too.
    async fn finish_run(
        &self,
        meeting_id: &str,
        outcome: RunResult,
        stop: &crate::diarize::Cancel,
        stood_down: &Arc<std::sync::atomic::AtomicBool>,
        stands_down: Option<Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<DiarizeOutcome> {
        use crate::diarize;

        // Nothing has been written on any of these paths, so the only question
        // left is whether the Meeting is still owed. A recording standing the
        // run down means it is; anything else means the Operator or a
        // disconnecting Client stopped it, and `diarize/cancel` has already
        // taken the row out.
        let stopped = || {
            if stood_down.load(std::sync::atomic::Ordering::SeqCst) {
                DiarizeOutcome::Paused
            } else {
                DiarizeOutcome::Cancelled
            }
        };

        let (diarization, rebuilt, prepared) = match outcome {
            // Not checked here: the only check that means anything is the one
            // inside the writer closure below, and a second one on this side of
            // the queue would just be an earlier answer to the same question.
            Ok(Ran {
                result: Ok(diarization),
                rebuilt,
                prepared,
            }) => (diarization, rebuilt, prepared),
            Ok(Ran {
                result: Err(diarize::DiarizeError::Cancelled),
                ..
            }) => return Ok(stopped()),
            Ok(Ran {
                result: Err(error), ..
            }) => {
                tracing::warn!(%error, "diarization did not run; the Meeting is unattributed");
                return Ok(DiarizeOutcome::Skipped);
            }
            Err(error) => {
                tracing::warn!(%error, "diarization failed; the Meeting is unattributed");
                return Ok(DiarizeOutcome::Skipped);
            }
        };

        // A re-seeding that could not be prepared is not a Meeting that
        // cannot be diarized. Skipping here would take it out of the line and
        // count it walked, which is the one thing a failure must not buy: the
        // backlog exists to recover these voices, and reporting success for a
        // Meeting whose evidence was never rebuilt loses them silently.
        let prepared = match prepared {
            Some(Err(error)) => {
                tracing::warn!(
                    %error,
                    "could not prepare this Meeting's voices for the re-run; it stays owed"
                );
                return Ok(DiarizeOutcome::Owed);
            }
            Some(Ok(prepared)) => Some(prepared),
            None => None,
        };

        let meeting_id = meeting_id.to_string();
        let stop_at_commit = stop.clone();
        let stood_down_at_commit = Arc::clone(stood_down);
        let written = self
            .store
            .write(move |connection| {
                // On the writer thread, with the connection in hand and no
                // transaction open: the last moment at which not writing is
                // still an option. A recording may have started while this
                // closure sat in the writer's queue, and the inference task
                // that was watching for one has long since ended, so the
                // eligibility is re-read here rather than trusted from before
                // the wait.
                stand_down_for_recording(
                    stands_down.as_ref(),
                    &stop_at_commit,
                    &stood_down_at_commit,
                );
                if stop_at_commit.is_cancelled() {
                    return Ok(Committed::Stopped);
                }
                // One transaction: a re-run withdraws the previous run's
                // evidence before it writes its own, and a Core that died
                // between the two would leave History knowing less than
                // either run had taught it.
                let transaction = connection.transaction()?;
                let adopted = diarize::cluster::adopt_rebuilt(
                    &transaction,
                    &rebuilt,
                    diarize::live::EMBEDDING_MODEL,
                    diarize::live::EMBEDDING_MODEL_VERSION,
                )?;
                if adopted.speakers > 0 {
                    tracing::info!(
                        rebuilt = adopted.rebuilt,
                        dropped = adopted.dropped,
                        speakers = adopted.speakers,
                        "Voiceprints rebuilt for the embedding model in use"
                    );
                }
                // The join first, then the Speakers. Persistence used to run
                // before reconciliation and minted a Speaker for every
                // cluster, words or none; now it is told which voices the
                // Transcript actually contains and mints only those.
                let segments = crate::store::meetings::segments(&transaction, &meeting_id)?;
                let reconciliation = diarize::reconcile::reconcile(&diarization, &segments);

                // The Operator's Voiceprint is admissible only in a Meeting
                // big enough for a match to mean anything, and below that
                // gate it leaves the resolve entirely rather than only the
                // flag — otherwise it sits among every other seed and
                // matches anyway, and the gate is decorative (ADR-0029 as
                // amended).
                // The re-run's own half, and every part of it is inside this
                // transaction on purpose: the bounded evidence, the
                // attribution it supports, and the queue row all commit or
                // none of them do. A failure below therefore cannot leave a
                // Meeting half-relearned, and a stop cannot leave evidence
                // behind for a walk that was never counted.
                let rebuilt_evidence = match prepared {
                    None => diarize::cluster::Rebuilt::default(),
                    Some(prepared) => {
                        // Membership is asked again here, on the writer. A
                        // bulk stop can land between the plan being read and
                        // this closure reaching the front of the writer's
                        // queue, and re-seeding a Meeting the backlog has
                        // given up on would write evidence for a walk nobody
                        // is counting.
                        if !crate::store::rerun::is_bulk_work(&transaction, &meeting_id)? {
                            return Ok(Committed::Owed);
                        }
                        // Before `apply` below, which replaces exactly what
                        // this reads: the previous model's attribution with
                        // the Operator's corrections on top.
                        let claims = diarize::cluster::claims(&transaction, &reconciliation)?;
                        let written = diarize::reseed::commit(
                            &transaction,
                            &prepared.plan,
                            &prepared.vectors,
                            diarize::live::EMBEDDING_MODEL,
                            diarize::live::EMBEDDING_MODEL_VERSION,
                        )?;
                        match written {
                            Ok(exemplars) => {
                                tracing::info!(
                                    exemplars,
                                    "relearned a Meeting's voices for the re-run"
                                );
                                diarize::cluster::Rebuilt {
                                    claimed: claims.claimed,
                                    reseeded: prepared.plan.owners.clone(),
                                }
                            }
                            // The record moved while the model was running —
                            // a correction, a rename, a forgetting. The
                            // vectors describe a world that no longer exists,
                            // and a fresh plan is the answer rather than
                            // writing them anyway or walking the Meeting
                            // without them. Nothing is committed, so the
                            // attribution this run computed goes too: it
                            // would otherwise be the half that landed.
                            Err(refused) => {
                                tracing::info!(
                                    ?refused,
                                    "the record moved while this Meeting was being relearned; it stays owed"
                                );
                                return Ok(Committed::Owed);
                            }
                        }
                    }
                };

                let gate = diarize::operator::match_gate_met(&diarization);
                let known = diarize::operator::known_operator(
                    &transaction,
                    diarize::live::EMBEDDING_MODEL,
                    diarize::live::EMBEDDING_MODEL_VERSION,
                )?;
                let withheld = (!gate).then(|| known.as_ref().map(|seed| seed.speaker_id.clone()));
                let withheld = withheld.flatten();

                let assigned = diarize::cluster::persist(
                    &transaction,
                    &meeting_id,
                    &diarization.embeddings,
                    &reconciliation.voices(),
                    withheld.as_deref(),
                    &rebuilt_evidence,
                )?;

                let facts = diarize::operator::MeetingFacts {
                    mic_isolated: crate::store::meetings::mic_isolated(&transaction, &meeting_id)?,
                };
                let found = diarize::operator::identify(
                    &diarization,
                    gate.then_some(known.as_ref()).flatten(),
                    &facts,
                );

                let written = diarize::reconcile::apply(
                    &transaction,
                    &reconciliation,
                    &assigned,
                    crate::store::speakers::Attribution::Clustered,
                )?;

                // "You", after the attributions are written, because
                // re-attaching moves segments and they have to exist first.
                let operator_id =
                    Self::attach_operator(&transaction, &found, &assigned, &meeting_id)?;

                // The other half of "a re-run replaces the run" (`persist`
                // did the first): the Speakers the previous run of this
                // Meeting minted and this one did not re-attribute now own
                // nothing, and go. Only here, after the segments moved —
                // before that they still owned this Meeting's words.
                let swept = crate::store::speakers::sweep_unreferenced(&transaction)?;
                // In the same transaction as the attribution it describes, so
                // a Meeting can never be marked diarized without the words
                // that marking is about.
                crate::store::meetings::set_diarized(&transaction, &meeting_id)?;
                // And out of the line here rather than in a second write
                // afterwards. That gap was a real one: between the commit and
                // a later removal, a bulk stop could find the row still there
                // and count a Meeting that had just been walked as one it
                // gave up on. Nothing outside this transaction can tell those
                // apart — a wall-clock stamp cannot, because two events a
                // ten-thousandth of a second apart round to the same
                // `julianday` and the clock can be adjusted under them — so
                // the gap is closed rather than measured.
                crate::store::diarize_queue::finish(&transaction, &meeting_id)?;
                transaction.commit()?;
                tracing::info!(
                    boundary_flips = reconciliation.boundary_flips,
                    attributed = reconciliation.attributed(),
                    voices = reconciliation.voices().len(),
                    speakers = assigned.len(),
                    operator = ?operator_id,
                    operator_rule = ?std::mem::discriminant(&found),
                    swept,
                    "diarization reconciled"
                );
                Ok(Committed::Wrote(written))
            })
            .await?;

        let written = match written {
            Committed::Wrote(written) => written,
            // The writer found a stop waiting for it. Nothing was written, so
            // the Meeting's state is what it was before this run.
            Committed::Stopped => return Ok(stopped()),
            // The transaction was dropped rather than committed, so this run
            // wrote nothing at all and the Meeting is owed a fresh pass.
            Committed::Owed => return Ok(DiarizeOutcome::Owed),
        };

        self.mirror_wake.notify_one();
        Ok(DiarizeOutcome::Wrote(written))
    }

    /// Points "You" at the Speakers this run identified as the Operator.
    ///
    /// **Re-attaches rather than mints.** Where a Speaker is already flagged
    /// it stays the Operator and this run's voices are folded into it. The
    /// old code set the flag on whatever Speaker the identified cluster had
    /// just been given, which was a fresh row whenever the Operator's
    /// Voiceprint had been deleted — so the flag landed on the new row, the
    /// lookup kept returning the old one, and the Registry showed two "You".
    ///
    /// A named Speaker is never folded away. Rule 1 names every mic-channel
    /// voice, and in a room where one of them is a colleague History already
    /// knows by name, resolving that contradiction by deleting the name
    /// would be the worst of the available answers.
    fn attach_operator(
        connection: &rusqlite::Connection,
        found: &crate::diarize::operator::Identified,
        assigned: &std::collections::BTreeMap<crate::diarize::Cluster, String>,
        meeting_id: &str,
    ) -> Result<Option<String>> {
        use crate::store::speakers;

        let mine: Vec<String> = found
            .clusters()
            .iter()
            .filter_map(|cluster| assigned.get(cluster).cloned())
            .collect();
        if mine.is_empty() {
            return Ok(None);
        }

        let target = match speakers::operator(connection)? {
            Some(existing) => existing.id,
            None => mine[0].clone(),
        };

        for speaker_id in &mine {
            if *speaker_id == target {
                continue;
            }
            let Some(source) = speakers::get(connection, speaker_id)? else {
                continue;
            };
            if source.display_name.is_some() {
                tracing::warn!(
                    meeting_id,
                    speaker_id,
                    "a named Speaker was identified as the Operator; leaving the name alone"
                );
                continue;
            }
            speakers::join(connection, speaker_id, &target)?;
        }

        speakers::set_operator(connection, &target)?;
        Ok(Some(target))
    }

    /// Works the Diarization queue until shutdown.
    ///
    /// One worker, so the "at most one run at a time" policy is a property
    /// of the shape rather than of a lock that every caller has to remember
    /// to take. `runner::Slot` stays underneath it: it is what releases on a
    /// panic out of the ONNX runtime, and what a direct call to
    /// `diarize_meeting` in a test still honours.
    ///
    /// A Meeting is taken out of the line only once its run is over, so a
    /// Core killed mid-run comes back owing it. That does mean a Meeting
    /// whose audio reliably panics the runtime would be retried on every
    /// start; it is bounded by `run_guarded` catching the panic and the run
    /// then finishing, unattributed, which takes it out of the line.
    ///
    /// **A recording stands the backlog down, and the backlog keeps its
    /// turn.** Two neural models and a capture pass want the same machine, so
    /// `Back` work — the model-change re-run, and the catch-up of Meetings a
    /// previous Core left — does not start while a Meeting records, and stops
    /// if one starts mid-pass. `Front` work does not yield: somebody is
    /// waiting for a Meeting that just ended, and Auto-Record opening the next
    /// call is not a reason to make them wait longer. A stood-down Meeting
    /// stays in the line, in the record, so it survives both the recording and
    /// a restart; it resumes when the recording ends, because stopping a
    /// Meeting queues it and that wakes this loop.
    ///
    /// Standing down is cooperative, and these are the only points that notice
    /// it, in order: before the run starts at all; inside the blocking task
    /// before decoding and again before the stale rebuild, both of which run
    /// ahead of the first progress tick and are expensive; at each window start
    /// inside `LiveDiarizer::observe`; at each progress tick; and finally
    /// inside the store's writer closure in [`Core::finish_run`], on the writer
    /// thread with the connection in hand and no transaction open yet. **No
    /// stage bounds the delay on its own** — model load, decode, the rebuild
    /// and the clustering pass all sit between consecutive checks, so "within
    /// one window" is not a guarantee this offers, and neither is interrupting
    /// a commit already under way. What it does guarantee is that a run stopped
    /// before that last check writes nothing.
    pub async fn run_diarization_queue(
        self: std::sync::Arc<Self>,
        shutdown: tokio_util::sync::CancellationToken,
    ) {
        // How many passes a Meeting still owed gets before it is treated as
        // unprocessable. Local to the worker because the worker is the only
        // thing that loops: a direct request gets the answer and decides for
        // itself.
        let mut owed: std::collections::BTreeMap<String, u8> = std::collections::BTreeMap::new();
        loop {
            let next = self
                .store
                .read(crate::store::diarize_queue::peek)
                .await
                .unwrap_or_else(|error| {
                    tracing::warn!(%error, "could not read the Diarization queue");
                    None
                });

            let Some((meeting_id, priority)) = next else {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = self.diarize_wake.notified() => continue,
                    // A timer as well as the notify: a wake that arrives
                    // while this loop is between selects is lost, and the
                    // cost of that should be a delay rather than a Meeting
                    // that waits until the next restart.
                    _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => continue,
                }
            };

            if shutdown.is_cancelled() {
                break;
            }

            let outcome = match self.diarize_meeting(&meeting_id, priority).await {
                Ok(outcome) => outcome,
                // Never fatal, and never the Meeting's problem: the record
                // stands whether or not anyone could be identified in it.
                // `Skipped`, because nothing reached a transaction and so
                // nothing has taken the row out — and it has to come out, or
                // this loop reads the same head, fails the same way and never
                // reaches the work behind it.
                Err(error) => {
                    tracing::warn!(meeting = %meeting_id, %error, "Diarization did not complete");
                    DiarizeOutcome::Skipped
                }
            };

            match outcome {
                DiarizeOutcome::Wrote(0) | DiarizeOutcome::Skipped | DiarizeOutcome::Cancelled => {}
                DiarizeOutcome::Wrote(attributed) => {
                    tracing::info!(meeting = %meeting_id, attributed, "Diarization attributed a Meeting")
                }
                DiarizeOutcome::Paused => {
                    tracing::debug!(meeting = %meeting_id, "a recording has the machine; the backlog waits")
                }
                DiarizeOutcome::Owed => {}
            }

            // A Meeting still owed keeps its place at the head of the queue,
            // so without a bound this loop would re-read it, re-infer and
            // fail the same way for ever. `Moved` is answered by a fresh plan
            // and almost always succeeds on the next pass — the correction
            // that moved it has landed by then — so one retry is what this
            // buys, and it costs one inference pass rather than an unbounded
            // number. Beyond that the Meeting is treated as the unprocessable
            // work it is behaving like, and leaves the line having written
            // nothing: its words, corrections and names all stand, and what
            // it loses is this Meeting's contribution to recognizing its
            // voices, exactly as a Meeting with no Kept Audio does.
            let outcome = if outcome == DiarizeOutcome::Owed {
                let attempts = owed.entry(meeting_id.clone()).or_insert(0);
                *attempts += 1;
                if *attempts >= RESEED_ATTEMPTS {
                    tracing::warn!(
                        meeting = %meeting_id,
                        attempts = *attempts,
                        "could not relearn this Meeting's voices; giving up on it"
                    );
                    owed.remove(&meeting_id);
                    DiarizeOutcome::Skipped
                } else {
                    outcome
                }
            } else {
                owed.remove(&meeting_id);
                outcome
            };

            if leaves_the_line_afterwards(outcome) {
                let done = meeting_id.clone();
                if let Err(error) = self
                    .store
                    .write(move |connection| crate::store::diarize_queue::finish(connection, &done))
                    .await
                {
                    // Left in the queue, so it is retried. Better than
                    // dropping it, and the alternative — spinning on a
                    // Meeting whose row cannot be deleted — needs the write
                    // path to be broken, which is a bigger problem than this
                    // loop.
                    tracing::warn!(meeting = %meeting_id, %error, "could not clear the Diarization queue");
                }
            }

            if matches!(outcome, DiarizeOutcome::Paused) {
                // Otherwise this loop spins on a Meeting it has just decided
                // not to run. `stop_meeting` queues the ended Meeting, which
                // notifies the wake, so the ordinary end of a recording
                // resumes the backlog at once; the timer is the same
                // belt-and-braces as above.
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = self.diarize_wake.notified() => {}
                    _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {}
                }
            }
        }
        tracing::debug!("diarization worker finished");
    }

    /// Puts a Meeting in line to be diarized. Answers whether it joined.
    ///
    /// Queued rather than run here, and queued in the record rather than in
    /// memory. Attribution arriving minutes later is the design (ADR-0009's
    /// join exists because the Transcript is already published), and
    /// anything that made stopping wait for two neural models would make the
    /// one act the Operator performs by hand feel broken.
    ///
    /// `false` means the Meeting was already in line. That is a refusal, and
    /// it is the caller's to report: M3's version refused inside the spawned
    /// task and only logged it, so the caller was told a run had started
    /// when none had.
    pub async fn enqueue_diarization(
        &self,
        meeting_id: &str,
        priority: crate::store::diarize_queue::Priority,
    ) -> Result<bool> {
        let id = meeting_id.to_string();
        let added = self
            .store
            .write(move |connection| {
                crate::store::diarize_queue::enqueue(connection, &id, priority)
            })
            .await?;
        self.diarize_wake.notify_one();
        Ok(added)
    }

    /// Puts the Meetings a previous Core never diarized back in line.
    ///
    /// **A detached run did not survive a restart, and nothing in the record
    /// said so.** M3 diarized in a detached task, so a Core stopping in those
    /// minutes took the run with it — leaving a `warn!` in a log the Operator
    /// cannot read and a Meeting whose words belong to nobody. Measured on
    /// the real History: two consecutive Meetings ended undiarized because an
    /// install swap restarted the Core twenty-three seconds after the second
    /// one stopped, and the first anyone knew was that every action item in
    /// their Summaries credited the unnamed-Speaker placeholder and was
    /// dropped (DECISIONS Q125).
    ///
    /// The queue already carries anything that reached it, and survives a
    /// restart because it lives in the record. This is for what never reached
    /// it: Meetings from before the queue existed, and ones whose run was
    /// claimed and lost. So it enqueues rather than diarizing here — one
    /// worker is the policy, and calling `diarize_meeting` directly would
    /// race it for `runner::Slot` and log `Busy` for everything it could not
    /// claim.
    ///
    /// `Back`, because a Meeting that just ended has somebody waiting for it
    /// and these have been waiting since a previous Core. Spawned rather than
    /// awaited, for the same reason the original was: booting must not wait.
    pub fn finish_interrupted_diarization(self: std::sync::Arc<Self>) {
        tokio::spawn(async move {
            let pending = match self
                .store
                .read(crate::store::meetings::never_diarized)
                .await
            {
                Ok(pending) => pending,
                Err(error) => {
                    warn!(%error, "could not look for Meetings that were never diarized");
                    return;
                }
            };
            if pending.is_empty() {
                return;
            }
            let mut queued = 0usize;
            for meeting_id in pending {
                match self
                    .enqueue_diarization(&meeting_id, crate::store::diarize_queue::Priority::Back)
                    .await
                {
                    Ok(true) => queued += 1,
                    // Already in line is the ordinary answer, not a problem.
                    Ok(false) => {}
                    Err(error) => warn!(
                        meeting = %meeting_id, %error,
                        "could not queue a Meeting a previous Core left"
                    ),
                }
            }
            if queued > 0 {
                info!(
                    meetings = queued,
                    "queued Diarization a previous Core did not finish"
                );
            }
        });
    }

    /// Whether a Meeting is running or waiting right now.
    pub async fn diarization_holds(&self, meeting_id: &str) -> Result<bool> {
        let id = meeting_id.to_string();
        self.store
            .read(move |connection| crate::store::diarize_queue::holds(connection, &id))
            .await
    }

    /// What Diarization is doing, and what it owes.
    pub async fn diarize_status(&self) -> Result<DiarizeStatusResponse> {
        // Read before the running job, so a queue drained between the two
        // reads cannot produce a status that shows neither.
        let queued = self
            .store
            .read(crate::store::diarize_queue::list)
            .await
            .unwrap_or_default();
        let rerun = self.rerun_block().await?;
        Ok(match self.diarization.lock().await.as_ref() {
            Some(job) => DiarizeStatusResponse {
                state: DiarizeState::Running,
                meeting_id: Some(job.meeting_id.clone()),
                done_ms: job.done_ms as i64,
                total_ms: job.total_ms as i64,
                queued,
                rerun,
            },
            None => DiarizeStatusResponse {
                state: if queued.is_empty() {
                    DiarizeState::Idle
                } else {
                    // Work is owed and the worker has not picked it up yet.
                    // Reported as running rather than idle: an Operator who
                    // asked for a re-run and saw "idle" would reasonably
                    // conclude nothing happened.
                    DiarizeState::Running
                },
                meeting_id: queued.first().cloned(),
                done_ms: 0,
                total_ms: 0,
                queued,
                rerun,
            },
        })
    }

    /// The bulk re-run's progress, when a backlog has been asked for.
    ///
    /// `None` for the two cases that have to keep the old wire shape exactly:
    /// a History whose re-run tables were never installed, which is every
    /// History in the field, and one whose first start merely wrote down
    /// which embedding it is in without asking for anything. Both of those
    /// are successes.
    ///
    /// Anything else is an error and is returned as one. Logging it and
    /// answering "no re-run" would tell the Client something false in the one
    /// shape it cannot question — a backlog that is running, reported as
    /// none at all.
    async fn rerun_block(&self) -> Result<Option<DiarizeRerun>> {
        let Some(state) = self.store.read(crate::store::rerun::state).await? else {
            return Ok(None);
        };
        if !state.requested() {
            return Ok(None);
        }
        Ok(Some(DiarizeRerun {
            total: state.total as i64,
            done: state.done() as i64,
            remaining: state.remaining as i64,
            abandoned: state.abandoned as i64,
            // A wait, not a stop: bulk work stands aside for a recording and
            // the Meetings are still owed. Reported separately from
            // `cancelled` for that reason.
            paused_for_recording: state.running() && self.is_recording().await,
            cancelled: state.cancelled,
            model: state.model,
            model_version: state.model_version,
        }))
    }

    /// Stops the bulk re-run, keeping every Meeting it already walked.
    ///
    /// Not routed through [`Core::diarize_cancel`]: that one takes a Meeting
    /// out of the line whatever put it there, which for a bulk stop would
    /// throw away the catch-up pass and anything an Operator is waiting for
    /// along with the backlog.
    ///
    /// **Whether the running job is this backlog's is decided on the writer,
    /// inside the mutation.** Asked beforehand on a read connection, the
    /// answer can be overtaken: a promotion to `Front` commits in between and
    /// the job is stopped anyway, on a Meeting somebody is now waiting for.
    /// So the running Meeting's name goes in and the answer comes back out,
    /// and the token is set on the writer thread before that thread takes any
    /// other work. A run short of its own writer closure therefore finds the
    /// token already cancelled and writes nothing, and one past it has
    /// committed and taken its own queue row out inside that same
    /// transaction, so this never sees it to miscount.
    ///
    /// The job lock is held across the whole of it, which keeps the handle
    /// the one the answer is about and stops a new job registering into the
    /// window between reading the queue and mutating it.
    pub async fn diarize_rerun_cancel(&self) -> Result<DiarizeStatusResponse> {
        {
            let running = self.diarization.lock().await;
            let name = running.as_ref().map(|job| job.meeting_id.clone());
            let token = running.as_ref().map(|job| job.cancel.clone());
            self.store
                .write(move |connection| {
                    let stopped = crate::store::rerun::cancel(connection, name.as_deref())?;
                    if stopped.stopped_active
                        && let Some(token) = token
                    {
                        token.cancel();
                    }
                    Ok(stopped)
                })
                .await?;
        }
        self.diarize_status().await
    }

    /// Stops a running Diarization, keeping whatever attribution completed.
    pub async fn diarize_cancel(&self, meeting_id: &str) -> Result<DiarizeStatusResponse> {
        // The running job holds a full id, so a short one would never match
        // and `cancel` would silently do nothing. An id that resolves to
        // nothing is passed through as typed — the queue is asked about it
        // and answers honestly — but a store that could not be read is an
        // error, not a reason to go on with a possibly-short id.
        let meeting_id = self
            .resolve_meeting(meeting_id)
            .await?
            .unwrap_or_else(|| meeting_id.to_string());
        {
            // The token and the removal under one hold of the job lock, the
            // same shape the bulk stop takes and for the same reason: apart,
            // a run can register between them and go on to write after the
            // Operator was told it had stopped. Registration takes this lock
            // and re-reads the queue, so whichever side gets here first wins
            // cleanly. No inference happens under it — the run is spawned
            // after registration, with the lock released.
            let running = self.diarization.lock().await;
            if let Some(job) = running.as_ref()
                && job.meeting_id == meeting_id
            {
                job.cancel.cancel();
            }
            // Out of the line as well as stopped. Cancelling a Meeting that
            // is still waiting has to mean it does not run — otherwise
            // "cancel" means "cancel, then run anyway in four minutes", which
            // is not a word anyone would choose for that. And giving up on
            // one of the bulk re-run's own Meetings is giving up, so the
            // backlog's books are settled in the same transaction rather than
            // left to report it as walked.
            let queued = meeting_id.clone();
            self.store
                .write(move |connection| crate::store::rerun::give_up(connection, &queued))
                .await?;
        }
        self.diarize_status().await
    }

    /// The Watchlist as Meeting Detection needs it.
    pub async fn watchlist_for_detection(&self) -> Result<crate::detect::watchlist::Watchlist> {
        self.store.read(crate::store::watchlist::load).await
    }

    /// Whether a Meeting is being recorded right now.
    pub async fn is_recording(&self) -> bool {
        self.recording.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// True once the Operator has acknowledged the Briefing here.
    pub async fn briefing_acknowledged(&self) -> bool {
        self.settings.lock().await.briefing_acknowledged
    }

    /// Replaces transcription. Tests use this to produce captions on demand.
    pub async fn set_transcriber_factory(&self, factory: TranscriberFactory) {
        *self.transcriber_factory.lock().await = Some(factory);
    }

    /// Subscribes to Core-raised notifications.
    pub fn notifications(&self) -> broadcast::Receiver<ServerNotification> {
        self.notifications.subscribe()
    }

    /// Replaces the capture source. Tests use this to drive the whole
    /// pipeline from a script instead of a microphone.
    /// Replaces the Backends a Summary run will use. Tests only.
    pub fn set_summary_backend_factory(&self, factory: SummaryBackendFactory) {
        *self
            .summary_backend_factory
            .lock()
            .expect("the summary backend factory mutex is never held across a panic") =
            Some(factory);
    }

    pub async fn set_source_factory(&self, factory: SourceFactory) {
        *self.source_factory.lock().await = factory;
    }

    /// Puts the record back in order after a Core that did not shut down
    /// cleanly.
    ///
    /// A killed Core leaves two things behind, and they are settled together
    /// because neither is readable without the other: checkpoints on disk,
    /// and a Meeting row that is still `active`.
    ///
    /// **The row was the half nobody collected.** Nothing consulted it at
    /// startup, so it stayed open — and a Meeting stays open until something
    /// stops it, which for an Auto-Recorded one is Detection noticing the app
    /// go away, possibly hours later. What the record then held was a Meeting
    /// that "ran" for as long as the app happened to stay open, with no
    /// audio, no transcript and no note: indistinguishable from a Meeting
    /// nobody spoke in, except that the duration was invented, and the
    /// duration is the part anyone reads. It also blocks the next recording,
    /// because `start_meeting_armed` refuses while one is active — so a Core
    /// killed once declines to record until something closes the row.
    ///
    /// Order matters. The active row is read *first*, because attaching
    /// recovered audio moves `updated_at`, and `updated_at` is the last
    /// moment there is any evidence the Meeting was alive — which is what it
    /// gets ended at, rather than now.
    pub async fn reconcile_after_restart(&self) {
        let interrupted = match self
            .store
            .read(|connection| {
                let Some(meeting) = meetings::active(connection)? else {
                    return Ok(None);
                };
                let touched = meetings::last_touched(connection, &meeting.id)?;
                Ok(Some((meeting, touched)))
            })
            .await
        {
            Ok(found) => found,
            Err(error) => {
                warn!(%error, "could not look for an interrupted Meeting");
                None
            }
        };

        let Some((meeting, last_touched)) = interrupted else {
            return;
        };

        // A killed Core leaves a playable file, not a directory of fragments:
        // MP3 is a frame stream, so what reached disk is already a recording
        // (ADR-0032). "Recovery" is therefore a question about one path — and
        // about the row, which is the half that used to be missed.
        let key = mirror::short_id(&meeting.id);
        let recovered = audio::sink::orphaned_audio(&self.audio_dir(), &key);
        if let Some((path, bytes)) = &recovered {
            let relative = self.relative_to_history(path);
            let id = meeting.id.clone();
            if let Err(error) = self
                .store
                .write(move |connection| meetings::set_audio_path(connection, &id, &relative))
                .await
            {
                warn!(%error, meeting = %meeting.id, "could not attach recovered audio");
            } else {
                info!(
                    meeting = %meeting.id,
                    seconds = audio::sink::seconds_from_bytes(*bytes),
                    "recovered audio and attached it to its Meeting"
                );
            }
        }

        // The note is the point. A Meeting that was cut short and says so is
        // a record; one that is merely short is a wrong one.
        let mut notes = meeting.audio_notes.clone();
        notes.push(match &recovered {
            Some(_) => "interrupted: the Core stopped without ending this Meeting. Its audio \
                        survives up to the moment it stopped, minus at most the final frame; \
                        any transcript still in flight was lost."
                .to_string(),
            None => "interrupted: the Core stopped without ending this Meeting, and no audio \
                     reached disk before it did."
                .to_string(),
        });

        let id = meeting.id.clone();
        let ended_at = interrupted_end(
            &meeting.started_at,
            last_touched,
            recovered.as_ref().map(|(_, bytes)| *bytes),
        );
        let result = self
            .store
            .write(move |connection| {
                meetings::set_audio_notes(connection, &id, &notes)?;
                meetings::stop_at(connection, &id, &ended_at)
            })
            .await;
        match result {
            Ok(_) => {
                warn!(
                    meeting = %meeting.id,
                    recovered = recovered.is_some(),
                    "closed a Meeting a previous Core left open"
                );
                self.wake_mirror();
            }
            Err(error) => warn!(%error, "could not close the interrupted Meeting"),
        }
    }

    fn audio_dir(&self) -> std::path::PathBuf {
        self.history_dir.join(paths::DATA_DIR_NAME).join("audio")
    }

    pub fn history_dir(&self) -> &std::path::Path {
        &self.history_dir
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn mirror(&self) -> &MirrorWriter {
        &self.mirror
    }

    pub fn mirror_wake(&self) -> Arc<Notify> {
        Arc::clone(&self.mirror_wake)
    }

    pub fn diarize_wake(&self) -> Arc<Notify> {
        Arc::clone(&self.diarize_wake)
    }

    pub fn uptime_seconds(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }

    pub async fn state(&self) -> CoreState {
        *self.state.lock().await
    }

    pub async fn set_state(&self, state: CoreState) {
        *self.state.lock().await = state;
    }

    async fn status(&self) -> StatusResponse {
        StatusResponse {
            version: evertranscript_protocol::VERSION.to_string(),
            pid: std::process::id(),
            uptime_seconds: self.uptime_seconds(),
            state: self.state().await,
            history_dir: self.history_dir.display().to_string(),
            incomplete_copy_warning: self.incomplete_copy.clone(),
        }
    }

    // ------------------------------------------------------------ Meetings

    /// Starts a Meeting. Refuses if one is already running: a Meeting runs
    /// record-start to record-stop, and two at once would make "the Meeting
    /// in progress" ambiguous for every other caller.
    pub async fn start_meeting(
        &self,
        title: Option<String>,
        detected_app: Option<String>,
    ) -> Result<Meeting> {
        self.start_meeting_armed(title, detected_app, None).await
    }

    /// Same, carrying the calendar event that named it (ADR-0036).
    pub async fn start_meeting_armed(
        &self,
        title: Option<String>,
        detected_app: Option<String>,
        armed: Option<crate::detect::CalendarEvent>,
    ) -> Result<Meeting> {
        // Nothing is captured before the Operator acknowledges the Briefing
        // (ADR-0023). This is the enforcement point rather than a UI
        // convention, so no Client — and no future Auto-Record path — can
        // route around it.
        if !self.briefing_acknowledged().await {
            anyhow::bail!(
                "recording is blocked until the first-run briefing is acknowledged \
                 (run `evertranscript acknowledge` or complete first-run setup)"
            );
        }

        let meeting = self
            .store
            .write(move |connection| {
                if let Some(running) = meetings::active(connection)? {
                    anyhow::bail!(
                        "a Meeting is already recording (started {})",
                        running.started_at
                    );
                }
                meetings::start_armed(
                    connection,
                    title.as_deref(),
                    detected_app.as_deref(),
                    armed.as_ref().map(|event| event.id.as_str()),
                    armed
                        .as_ref()
                        .map(|event| event.attendees.as_slice())
                        .unwrap_or(&[]),
                )
            })
            .await?;

        // Capture starts after the Meeting exists, so a recording can never
        // be running without a row to attach it to.
        let source = (self.source_factory.lock().await)();
        let script = self.settings.lock().await.chinese_script;
        let (segments_tx, segments_rx) = mpsc::channel(256);
        // No engine means no captions at all, so the channel and the script
        // go with it rather than being carried alongside a `None`.
        let captions = self
            .open_transcriber()
            .await
            .map(|transcriber| audio::recorder::Captions {
                transcriber,
                segments: segments_tx,
                script,
            });

        match audio::recorder::Recorder::start(
            source,
            self.audio_dir(),
            mirror::short_id(&meeting.id),
            captions,
        ) {
            Ok(recorder) => {
                *self.recorder.lock().await = Some(recorder);
                self.recording
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                // Segments are persisted by their own task, so a slow disk
                // slows the transcript rather than the recording.
                tokio::spawn(write_segments(
                    self.store.clone(),
                    Arc::clone(&self.mirror_wake),
                    self.notifications.clone(),
                    meeting.id.clone(),
                    segments_rx,
                ));
            }
            Err(error) => {
                // The transcript is the record; audio is the bonus
                // (ADR-0019). A Meeting with no audio still beats no Meeting.
                warn!(%error, "capture could not start; recording without audio");
            }
        }

        self.set_state(CoreState::Recording).await;
        self.wake_mirror();
        Ok(meeting)
    }

    /// Stops the Meeting in progress and persists it (story 5).
    pub async fn stop_meeting(&self) -> Result<Meeting> {
        let meeting = self
            .store
            .write(|connection| {
                let Some(running) = meetings::active(connection)? else {
                    anyhow::bail!("no Meeting is recording");
                };
                meetings::stop(connection, &running.id)
            })
            .await?;

        // Finalize capture before answering: "stopped" must mean the audio
        // is merged and on disk, not merged eventually.
        if let Some(recorder) = self.recorder.lock().await.take() {
            self.recording
                .store(false, std::sync::atomic::Ordering::SeqCst);
            let outcome = recorder.finish().await;
            if let Some(path) = &outcome.audio_path {
                let relative = self.relative_to_history(path);
                let id = meeting.id.clone();
                self.store
                    .write(move |connection| meetings::set_audio_path(connection, &id, &relative))
                    .await?;
            }
            // What the recording knows and the audio does not: whether the
            // far end could have reached the microphone. Written before
            // Diarization is queued below, because that run is what reads
            // it (ADR-0029's first rule).
            if let Some(isolated) = outcome.mic_isolated {
                let id = meeting.id.clone();
                self.store
                    .write(move |connection| meetings::set_mic_isolated(connection, &id, isolated))
                    .await?;
            }
            if !outcome.degraded.is_empty() {
                for note in &outcome.degraded {
                    warn!(meeting = %meeting.id, note, "this Meeting's audio is partial");
                }
                // And into the record. A log line is invisible to the person
                // who later reads a transcript with one side missing.
                let id = meeting.id.clone();
                let notes = outcome.degraded.clone();
                self.store
                    .write(move |connection| meetings::set_audio_notes(connection, &id, &notes))
                    .await?;
            }
        }

        self.set_state(CoreState::Idle).await;
        self.wake_mirror();
        // Persisting means the Mirror exists too, not just the rows.
        self.mirror.rebuild_pending().await?;

        // Queued here rather than in the `meeting/stop` handler, because
        // three paths stop a Meeting and only one of them went through that
        // handler: Auto-Record's driver and the tray both call this
        // directly, and neither ever diarized what it stopped. This is where
        // all three converge.
        //
        // At the front: somebody just finished a call, and an overnight
        // re-run of History must not put them behind sixteen others. After
        // the audio is merged and on disk, because the run needs it.
        //
        // A failure to queue is not a failure to stop. The Operator pressed
        // a button; attribution is the part that can be asked for again.
        if let Err(error) = self
            .enqueue_diarization(&meeting.id, diarize_queue::Priority::Front)
            .await
        {
            warn!(meeting = %meeting.id, %error, "could not queue Diarization for a stopped Meeting");
        }

        self.get_meeting(&meeting.id)
            .await?
            .map(|(meeting, _)| meeting)
            .ok_or_else(|| anyhow::anyhow!("the Meeting vanished after stopping"))
    }

    /// Paths are stored relative to the History folder so the record stays
    /// portable: moving the folder must not break every audio reference
    /// (ADR-0035).
    fn relative_to_history(&self, path: &std::path::Path) -> String {
        Self::history_relative(&self.history_dir, path)
    }

    /// A path under the History folder, as the record stores it.
    ///
    /// **Forward slashes on every platform, and that is about ADR-0035 rather
    /// than taste.** The History folder is the Operator's record and the complete
    /// portable unit — the thing they copy to another machine, sync, or hand to
    /// someone. `Path::display` writes the *host's* separator, so a Meeting
    /// recorded on Windows stored `.data\audio\01a074b1.mp3`, which resolves on
    /// Windows and nowhere else. The reverse direction was always fine, because
    /// Windows accepts forward slashes too — which is exactly why this went
    /// unnoticed: every macOS-written record opened correctly on Windows.
    ///
    /// Found when the Windows CI job got far enough to run `capture_vertical`
    /// for the first time, which it could not do while an earlier test was
    /// failing ahead of it.
    ///
    /// Records already written on Windows keep their backslashes and keep
    /// working there; nothing rewrites the Operator's rows behind their back.
    pub(crate) fn history_relative(
        history_dir: &std::path::Path,
        path: &std::path::Path,
    ) -> String {
        let relative = path.strip_prefix(history_dir).unwrap_or(path);
        relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    }

    pub async fn list_meetings(&self, limit: u32, offset: u32) -> Result<Vec<Meeting>> {
        self.store
            .read(move |connection| meetings::list(connection, limit, offset))
            .await
    }

    /// The Meeting in progress with its transcript so far, if one is running.
    pub async fn current_meeting_with_transcript(
        &self,
    ) -> Result<Option<(Meeting, Vec<TranscriptSegment>)>> {
        let running = self.store.read(meetings::active).await?;
        match running {
            Some(meeting) => self.get_meeting(&meeting.id).await,
            None => Ok(None),
        }
    }

    /// The stored Meeting id for what a caller passed.
    ///
    /// **Every method taking a client-supplied id starts here**, so the short
    /// form `evertranscript list` prints resolves everywhere rather than only
    /// where somebody remembered. Resolution happens once, at the top, and
    /// what flows downstream is always the full id — which is what keeps a
    /// half-resolved id out of a `DELETE`.
    async fn resolve_meeting(&self, typed: &str) -> Result<Option<String>> {
        let typed = typed.to_string();
        self.store
            .read(move |connection| meetings::resolve(connection, &typed))
            .await
    }

    async fn resolve_speaker(&self, typed: &str) -> Result<Option<String>> {
        let typed = typed.to_string();
        self.store
            .read(move |connection| crate::store::speakers::resolve(connection, &typed))
            .await
    }

    pub async fn get_meeting(&self, id: &str) -> Result<Option<(Meeting, Vec<TranscriptSegment>)>> {
        let Some(id) = self.resolve_meeting(id).await? else {
            return Ok(None);
        };
        self.store
            .read(move |connection| {
                let Some(meeting) = meetings::get(connection, &id)? else {
                    return Ok(None);
                };
                Ok(Some((meeting, meetings::segments(connection, &id)?)))
            })
            .await
    }

    pub async fn retitle_meeting(&self, id: &str, title: &str) -> Result<Meeting> {
        let id = self
            .resolve_meeting(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no Meeting with id {id}"))?;
        let title = title.to_string();
        let meeting = self
            .store
            .write(move |connection| meetings::retitle(connection, &id, &title))
            .await?;
        // The filename follows the title, so rebuild before answering: the
        // caller's next `ls` should already show the new name.
        self.mirror.rebuild_pending().await?;
        self.get_meeting(&meeting.id)
            .await?
            .map(|(meeting, _)| meeting)
            .ok_or_else(|| anyhow::anyhow!("the Meeting vanished after retitling"))
    }

    /// Removes a Meeting entirely: rows, Mirror, and audio (story 21).
    pub async fn delete_meeting(&self, id: &str) -> Result<bool> {
        // Resolved before a single row goes. `resolve` refuses an ambiguous
        // prefix rather than picking one, which is the whole reason it does
        // not guess: this call takes the audio with it.
        let Some(id) = self.resolve_meeting(id).await? else {
            return Ok(false);
        };
        let id = id.as_str();
        let id_for_write = id.to_string();
        let deleted = self
            .store
            .write(move |connection| {
                let transaction = connection.transaction()?;
                let deleted = meetings::delete(&transaction, &id_for_write)?;
                transaction.commit()?;
                Ok(deleted)
            })
            .await?;

        if !deleted.existed {
            return Ok(false);
        }
        if let Some(filename) = deleted.mirror_filename {
            self.mirror.remove(&filename);
        }
        if let Some(audio_path) = deleted.audio_path {
            let path = self.history_dir.join(&audio_path);
            if let Err(error) = std::fs::remove_file(&path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                warn!(path = %path.display(), %error, "could not remove the Meeting's audio");
            }
        }
        Ok(true)
    }

    /// The Meeting's Mirror markdown. Rendered from the record rather than
    /// read back from disk, so an export is never a stale file.
    pub async fn export_meeting(&self, id: &str) -> Result<Option<(String, Option<String>)>> {
        let Some((meeting, segments)) = self.get_meeting(id).await? else {
            return Ok(None);
        };
        let names = self
            .store
            .read(|connection| {
                let entries = crate::store::speakers::list(connection)?
                    .into_iter()
                    .map(|speaker| {
                        (
                            speaker.id,
                            mirror::SpeakerName {
                                display_name: speaker.display_name,
                                is_operator: speaker.is_operator,
                            },
                        )
                    })
                    .collect();
                Ok(mirror::SpeakerNames::from_entries(entries))
            })
            .await?;
        let markdown = mirror::render(&meeting, &segments, &names);
        let path = meeting
            .mirror_filename
            .as_ref()
            .map(|filename| self.history_dir.join(filename).display().to_string());
        Ok(Some((markdown, path)))
    }

    pub async fn search_history(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<evertranscript_protocol::SearchResult>> {
        let query = query.to_string();
        self.store
            .read(move |connection| meetings::search(connection, &query, limit))
            .await
    }

    /// Appends a Transcript segment to the Meeting in progress. Ticket 06's
    /// ASR pipeline is the real caller; it exists here so the storage path is
    /// exercised end to end before then.
    pub async fn append_segment(
        &self,
        meeting_id: &str,
        channel: AudioChannel,
        start_ms: i64,
        end_ms: i64,
        text: &str,
    ) -> Result<TranscriptSegment> {
        let (meeting_id, text) = (meeting_id.to_string(), text.to_string());
        let segment = self
            .store
            .write(move |connection| {
                meetings::append_segment(connection, &meeting_id, channel, start_ms, end_ms, &text)
            })
            .await?;
        self.wake_mirror();
        Ok(segment)
    }

    fn wake_mirror(&self) {
        self.mirror_wake.notify_one();
    }

    /// Loads the transcription engine, or reports why there isn't one.
    ///
    /// A missing model degrades to "record without captions" rather than
    /// refusing to record: never missing a meeting outranks transcribing it
    /// live (ADR-0019, ADR-0023).
    async fn open_transcriber(&self) -> Option<Box<dyn crate::asr::Transcriber>> {
        if let Some(factory) = self.transcriber_factory.lock().await.as_ref() {
            return factory();
        }
        let downloader = models::Downloader::new(self.models_dir.clone()).ok()?;
        let entry = &models::registry::WHISPER_DEFAULT;
        let models::ModelStatus::Ready { path } = downloader.status(entry) else {
            warn!(
                model = entry.key,
                "no transcription model yet; recording without live captions. \
                 Run `evertranscript models fetch`."
            );
            return None;
        };
        match tokio::task::spawn_blocking(move || crate::asr::whisper::WhisperEngine::load(&path))
            .await
        {
            Ok(Ok(engine)) => Some(Box::new(engine) as Box<dyn crate::asr::Transcriber>),
            Ok(Err(error)) => {
                warn!(%error, "the transcription model failed to load; recording without captions");
                None
            }
            Err(error) => {
                warn!(%error, "loading the transcription model panicked");
                None
            }
        }
    }

    // -------------------------------------------------------------- Models

    /// What is on disk and what is still needed. Never touches the network.
    pub fn models_status(&self) -> Result<ModelsStatusResponse> {
        let downloader = models::Downloader::new(self.models_dir.clone())?;
        let models: Vec<ModelState> = models::registry::ALL
            .iter()
            .map(|entry| describe_model(&downloader, entry))
            .collect();
        let ready = models
            .iter()
            .all(|model| !model.required || model.state == ModelAvailability::Ready);
        Ok(ModelsStatusResponse { models, ready })
    }

    /// Fetches what a fresh install is missing, if this is one.
    ///
    /// **Called by the binary, never by construction.** A Core that is built
    /// and never asked provisions nothing — which is what lets the guarantee
    /// tests keep building fresh Cores against isolated directories and
    /// asserting no socket ever opens. Suppressing an implicit fetch with a
    /// test-only switch would have proved that guarantee only with this
    /// feature disabled.
    ///
    /// Returns what it decided, so a caller can say so rather than guess.
    /// Records Local as the Summary Backend on a first start.
    ///
    /// **Preselected, not defaulted.** ADR-0013 was written when neither
    /// option was obviously right — Local meant a model that might be absent
    /// and, when present, invented action items. With a provisioned model
    /// that measures well, an Operator who has no basis for the choice is
    /// better served by a working configuration than by a disabled Continue.
    ///
    /// The value is *written* rather than inferred from absence, so a running
    /// configuration still traces to something rather than to a `None` that
    /// means two different things. Choosing Cloud is untouched: still
    /// deliberate, still behind its one-time warning.
    /// Removes models this build superseded. Safe to call on every start.
    pub fn remove_superseded_models(&self) {
        for filename in models::remove_superseded(&self.models_dir) {
            info!(filename, "removed a model this build no longer loads");
        }
    }

    pub async fn preselect_local_backend(&self) -> Result<()> {
        let mut settings = self.settings.lock().await;
        // Never clobber a choice. An Operator who chose Cloud must not be
        // reset to Local by installing a newer version.
        if settings.summary_backend.is_some() {
            return Ok(());
        }
        settings.summary_backend = Some("local".to_string());
        settings.save_to(&self.settings_path)?;
        info!("preselected the local Summary Backend for a fresh install");
        Ok(())
    }

    pub async fn provision_missing_models(
        &self,
        cancel: CancellationToken,
    ) -> Result<models::provision::Provision> {
        let status = self.models_status()?;
        let missing: Vec<&models::registry::ModelEntry> = models::registry::ALL
            .iter()
            .filter(|entry| entry.required)
            .filter(|entry| {
                status
                    .models
                    .iter()
                    .any(|model| model.key == entry.key && model.state != ModelAvailability::Ready)
            })
            .collect();

        let machine = models::provision::Machine {
            models_present: missing.is_empty(),
            free_bytes: models::free_space_bytes(&self.models_dir),
            needed_bytes: missing.iter().map(|entry| entry.integrity.size_bytes).sum(),
        };

        let decision = models::provision::decide(machine);
        match decision {
            models::provision::Provision::Fetch => {
                let total: u64 = machine.needed_bytes;
                // Said before it starts, not after. An automatic transfer of
                // this size that nobody was told about is the surprise
                // Nothing Ambient exists to prevent.
                info!(
                    megabytes = total / 1_048_576,
                    models = missing.len(),
                    "fetching the models this install is missing"
                );
                self.fetch_models(None, cancel).await?;
            }
            models::provision::Provision::NotEnoughSpace {
                free_bytes,
                needed_bytes,
            } => {
                warn!(
                    free_megabytes = free_bytes / 1_048_576,
                    needed_megabytes = needed_bytes / 1_048_576,
                    "not provisioning: there is not enough room"
                );
            }
            models::provision::Provision::NothingMissing => {}
        }
        Ok(decision)
    }

    /// Makes the login item match the setting that claims it exists.
    ///
    /// `launch_at_login` defaults to true, and registration only ever
    /// happened when a Client explicitly *set* it — which onboarding never
    /// does, since its steps are the Briefing, permissions, models, folder,
    /// Backend and calendar. So a fresh install carried a setting that said
    /// "on" and no login item at all, and the Core that `CONTEXT.md` defines
    /// as "the always-on process — the login item" did not start at login.
    /// The CLI noticed and told the Operator to run a command; nothing acted.
    ///
    /// Reconciled in both directions, because drift the other way is just as
    /// wrong: a registration left behind by a setting since turned off would
    /// start a Core the Operator asked not to have.
    pub async fn reconcile_login_item(&self) {
        if autostart::disabled() {
            return;
        }
        let wanted = self.settings.lock().await.launch_at_login;
        if autostart::is_enabled() == wanted {
            return;
        }
        match autostart::set_enabled(wanted) {
            Ok(()) => info!(
                registered = wanted,
                "the login item did not match the setting; reconciled"
            ),
            Err(error) => warn!(%error, "could not reconcile the login item"),
        }
    }

    /// Keeps trying until every required model is present, or shutdown.
    ///
    /// One attempt is not enough for a promise that the product will work:
    /// the first start after an install is exactly when a laptop is most
    /// likely to be on a hotel network, asleep, or tethered, and a fetch that
    /// failed there used to wait for the next launch — which on a machine that
    /// stays logged in is never. Downloads resume rather than restart, so a
    /// retry costs only what the last one did not finish.
    ///
    /// Backs off to half an hour. A missing model is not urgent enough to
    /// retry hard, and a machine that is offline for a day should not spend it
    /// asking.
    pub async fn provision_until_complete(&self, cancel: CancellationToken) {
        const FIRST: std::time::Duration = std::time::Duration::from_secs(60);
        const LONGEST: std::time::Duration = std::time::Duration::from_secs(30 * 60);

        if models::provision::fetching_disabled() {
            info!(
                "not fetching models: {} is set",
                models::provision::DISABLE_ENV
            );
            return;
        }

        let mut wait = FIRST;
        loop {
            match self.provision_missing_models(cancel.clone()).await {
                Ok(models::provision::Provision::NothingMissing) => return,
                Ok(other) => debug!(?other, "models are still missing; will look again"),
                Err(error) => {
                    warn!(%error, "fetching models did not finish; will try again")
                }
            }
            if cancel.is_cancelled() {
                return;
            }
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tokio::time::sleep(wait) => {}
            }
            wait = (wait * 2).min(LONGEST);
        }
    }

    /// Downloads what is missing. A corrupted file is removed first so the
    /// fetch starts from a clean slate rather than trying to resume garbage.
    pub async fn fetch_models(&self, key: Option<&str>, cancel: CancellationToken) -> Result<()> {
        *self
            .fetching
            .lock()
            .expect("the fetch token mutex is never held across a panic") = Some(cancel.clone());
        let result = self.fetch_models_inner(key, cancel).await;
        *self
            .fetching
            .lock()
            .expect("the fetch token mutex is never held across a panic") = None;
        result
    }

    /// Stops a fetch in flight. Partial files stay, so asking again resumes.
    pub fn cancel_fetch(&self) {
        if let Some(token) = self
            .fetching
            .lock()
            .expect("the fetch token mutex is never held across a panic")
            .as_ref()
        {
            token.cancel();
        }
    }

    async fn fetch_models_inner(&self, key: Option<&str>, cancel: CancellationToken) -> Result<()> {
        let downloader = models::Downloader::new(self.models_dir.clone())?;
        let entries: Vec<&'static models::registry::ModelEntry> = match key {
            Some(key) => vec![
                models::registry::find(key)
                    .ok_or_else(|| anyhow::anyhow!("no model with key {key}"))?,
            ],
            None => models::registry::required().collect(),
        };

        for entry in entries {
            if let models::ModelStatus::Corrupted { reason } = downloader.status(entry) {
                warn!(model = entry.key, reason, "discarding a corrupted model");
                downloader.remove(entry)?;
            }
            // Broadcast, not merely logged. An Operator cannot read the
            // Core's log, and this download starts without being asked.
            let outcome = downloader
                .fetch(entry, cancel.clone(), |progress| {
                    debug!(
                        model = entry.key,
                        percent = (progress.fraction() * 100.0) as u32,
                        "downloading"
                    );
                    let _ = self.notifications.send(ServerNotification::ModelProgress(
                        evertranscript_protocol::ModelProgressParams {
                            key: entry.key.to_string(),
                            display_name: entry.display_name.to_string(),
                            done_bytes: progress.downloaded_bytes as i64,
                            total_bytes: progress.total_bytes as i64,
                            stopped: None,
                        },
                    ));
                })
                .await;
            if let Err(error) = outcome {
                // A stopped download is a different thing from one still
                // running at the same byte count, and a Client showing a
                // frozen bar cannot tell them apart.
                let _ = self.notifications.send(ServerNotification::ModelProgress(
                    evertranscript_protocol::ModelProgressParams {
                        key: entry.key.to_string(),
                        display_name: entry.display_name.to_string(),
                        done_bytes: 0,
                        total_bytes: entry.integrity.size_bytes as i64,
                        stopped: Some(error.to_string()),
                    },
                ));
                return Err(error);
            }
        }
        Ok(())
    }
}

fn describe_model(
    downloader: &models::Downloader,
    entry: &models::registry::ModelEntry,
) -> ModelState {
    let (state, bytes_on_disk, path, detail) = match downloader.status(entry) {
        models::ModelStatus::Missing => (ModelAvailability::Missing, None, None, None),
        models::ModelStatus::Partial { bytes_on_disk } => {
            (ModelAvailability::Partial, Some(bytes_on_disk), None, None)
        }
        models::ModelStatus::Corrupted { reason } => {
            (ModelAvailability::Corrupted, None, None, Some(reason))
        }
        models::ModelStatus::Ready { path } => (
            ModelAvailability::Ready,
            Some(entry.integrity.size_bytes),
            Some(path.display().to_string()),
            None,
        ),
    };
    ModelState {
        key: entry.key.to_string(),
        display_name: entry.display_name.to_string(),
        state,
        required: entry.required,
        total_bytes: entry.integrity.size_bytes,
        bytes_on_disk,
        path,
        detail,
    }
}

/// A request's result, as the message that answers it.
fn reply(id: RequestId, result: Result<serde_json::Value>) -> JsonRpcMessage {
    match result {
        Ok(result) => JsonRpcMessage::Response(JsonRpcResponse { id, result }),
        Err(error) => JsonRpcMessage::Error(JsonRpcError::new(
            id,
            error_codes::INTERNAL_ERROR,
            error.to_string(),
        )),
    }
}

/// Per-connection state. Evaporates when the Client disconnects; the record
/// and any in-flight work do not.
struct Connection {
    writer: mpsc::Sender<JsonRpcMessage>,
    initialized: bool,
    experimental_api: bool,
    /// Captions are opt-in: a CLI running `search` should not be sent every
    /// word of a live meeting.
    captions: bool,
    /// Captions dropped because this connection was not keeping up, so the
    /// Client can be told it has a gap rather than silently missing words.
    captions_dropped: u32,
}

/// Runs the protocol for every attached Client.
pub struct Server {
    core: Arc<Core>,
    connections: HashMap<ConnectionId, Connection>,
}

impl Server {
    pub fn new(core: Arc<Core>) -> Self {
        Self {
            core,
            connections: HashMap::new(),
        }
    }

    pub fn core(&self) -> &Arc<Core> {
        &self.core
    }

    /// Consumes transport events until the channel closes or shutdown fires.
    pub async fn run(
        mut self,
        mut events: mpsc::Receiver<TransportEvent>,
        shutdown: CancellationToken,
    ) {
        let mut core_notifications = self.core.notifications();
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                event = events.recv() => match event {
                    Some(event) => self.handle_event(event).await,
                    None => break,
                },
                notification = core_notifications.recv() => match notification {
                    Ok(notification) => self.fan_out(notification).await,
                    // Lagged: the Core produced faster than this loop
                    // consumed. Captions are lossy by design (ADR-0028), so
                    // the gap is reported and the stream continues.
                    Err(broadcast::error::RecvError::Lagged(count)) => {
                        warn!(count, "the server fell behind on Core notifications");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        core_notifications = self.core.notifications();
                    }
                },
            }
        }
        debug!("server loop finished");
    }

    /// Routes a Core notification to the connections that asked for it.
    async fn fan_out(&mut self, notification: ServerNotification) {
        match notification {
            // Captions go only to subscribers, and only lossily.
            ServerNotification::TranscriptSegmentAdded(_) => {
                self.broadcast_captions(notification).await
            }
            other => self.broadcast(other).await,
        }
    }

    async fn handle_event(&mut self, event: TransportEvent) {
        match event {
            TransportEvent::Opened {
                connection_id,
                writer,
            } => {
                self.connections.insert(
                    connection_id,
                    Connection {
                        writer,
                        initialized: false,
                        experimental_api: false,
                        captions: false,
                        captions_dropped: 0,
                    },
                );
            }
            TransportEvent::Closed { connection_id } => {
                self.connections.remove(&connection_id);
            }
            TransportEvent::Line {
                connection_id,
                line,
            } => {
                self.handle_line(connection_id, &line).await;
            }
        }
    }

    async fn handle_line(&mut self, connection_id: ConnectionId, line: &str) {
        let message: JsonRpcMessage = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(err) => {
                warn!(connection_id, %err, "unparseable line from client");
                // Without an id there is nobody to answer; the line is dropped.
                return;
            }
        };

        match message {
            JsonRpcMessage::Request(request) => {
                if let Some(response) = self
                    .dispatch_request(
                        connection_id,
                        request.id.clone(),
                        &request.method,
                        request.params,
                    )
                    .await
                {
                    self.send(connection_id, response).await;
                }
            }
            JsonRpcMessage::Notification(notification) => {
                match ClientNotification::from_wire(&notification.method, notification.params) {
                    Ok(ClientNotification::Initialized(_)) => {
                        debug!(connection_id, "client finished initializing");
                    }
                    Err(err) => {
                        debug!(connection_id, %err, "ignoring unknown client notification");
                    }
                }
            }
            JsonRpcMessage::Response(_) | JsonRpcMessage::Error(_) => {
                // Server-to-client requests do not exist yet, so a response
                // arriving here has nothing to correlate with.
                debug!(connection_id, "ignoring unexpected response from client");
            }
        }
    }

    async fn dispatch_request(
        &mut self,
        connection_id: ConnectionId,
        id: RequestId,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Option<JsonRpcMessage> {
        let request = match ClientRequest::from_wire(method, params) {
            Ok(request) => request,
            Err(evertranscript_protocol::DecodeError::UnknownMethod(method)) => {
                return Some(JsonRpcMessage::Error(JsonRpcError::new(
                    id,
                    error_codes::METHOD_NOT_FOUND,
                    format!("unknown method: {method}"),
                )));
            }
            Err(err) => {
                return Some(JsonRpcMessage::Error(JsonRpcError::new(
                    id,
                    error_codes::INVALID_PARAMS,
                    err.to_string(),
                )));
            }
        };

        let initialized = self
            .connections
            .get(&connection_id)
            .is_some_and(|connection| connection.initialized);

        match (&request, initialized) {
            (ClientRequest::Initialize(_), true) => {
                return Some(JsonRpcMessage::Error(JsonRpcError::new(
                    id,
                    error_codes::ALREADY_INITIALIZED,
                    "this connection is already initialized",
                )));
            }
            (request, false) if !matches!(request, ClientRequest::Initialize(_)) => {
                return Some(JsonRpcMessage::Error(JsonRpcError::new(
                    id,
                    error_codes::NOT_INITIALIZED,
                    "send initialize before any other request",
                )));
            }
            _ => {}
        }

        // Four requests can take minutes: a model download, a microphone
        // check, a Summary, and the Calendars prompt, which waits up to five
        // for an answer. This loop answers every Client and forwards every
        // notification, so those four are answered from tasks of their own.
        // Everything else is answered here, in the order it arrived.
        let core = Arc::clone(&self.core);
        match request {
            ClientRequest::ModelsFetch(params) => {
                self.answer_later(connection_id, id, async move {
                    core.fetch_models(params.key.as_deref(), CancellationToken::new())
                        .await?;
                    Ok(serde_json::to_value(core.models_status()?)?)
                })
            }

            // Recording, on the Core, for as long as the caller asked. It is
            // the Core that records in production, so it is the Core that has
            // to be the one asked — a check the Client ran in its own process
            // would prove that Electron can reach a microphone and nothing
            // about the process that actually captures Meetings.
            ClientRequest::AudioCheck(params) => {
                let seconds = params
                    .seconds
                    .unwrap_or(audio::check::DEFAULT_SECONDS)
                    .clamp(1, 120);
                self.answer_later(connection_id, id, async move {
                    Ok(serde_json::to_value(audio::check::run(seconds).await)?)
                })
            }

            ClientRequest::SummaryGenerate(params) => {
                self.answer_later(connection_id, id, async move {
                    core.summarize_meeting(&params.id).await?;
                    let meeting = core
                        .get_meeting(&params.id)
                        .await?
                        .map(|(meeting, _)| meeting)
                        .ok_or_else(|| anyhow::anyhow!("the Meeting vanished"))?;
                    // `announce` belongs to the loop; the Core's own channel
                    // reaches the same Clients through it.
                    let _ = core.notifications.send(ServerNotification::MeetingChanged(
                        MeetingChangedParams {
                            kind: MeetingChangeKind::Updated,
                            meeting_id: meeting.id.clone(),
                            meeting: Some(meeting.clone()),
                        },
                    ));
                    Ok(serde_json::to_value(MeetingResponse { meeting })?)
                })
            }

            ClientRequest::CalendarRequestAccess(_) => {
                self.answer_later(connection_id, id, async move {
                    Ok(serde_json::to_value(core.request_calendar_access().await?)?)
                })
            }

            request => return Some(reply(id, self.handle(connection_id, request).await)),
        }
        None
    }

    /// Answers a request from a task of its own, so the loop goes on serving
    /// every other Client meanwhile.
    fn answer_later(
        &self,
        connection_id: ConnectionId,
        id: RequestId,
        work: impl Future<Output = Result<serde_json::Value>> + Send + 'static,
    ) {
        // Always there: only an initialized connection gets this far.
        let Some(connection) = self.connections.get(&connection_id) else {
            return;
        };
        let writer = connection.writer.clone();
        tokio::spawn(async move {
            // A Client that left meanwhile has nobody to tell, and the loop
            // drops its connection when the transport reports it closed.
            let _ = writer.send(reply(id, work.await)).await;
        });
    }

    async fn handle(
        &mut self,
        connection_id: ConnectionId,
        request: ClientRequest,
    ) -> Result<serde_json::Value> {
        match request {
            ClientRequest::Initialize(params) => Ok(self.handle_initialize(connection_id, params)),

            ClientRequest::Status(_) => Ok(serde_json::to_value(self.core.status().await)?),

            ClientRequest::MeetingStart(params) => {
                let meeting = self
                    .core
                    .start_meeting(params.title, params.detected_app)
                    .await?;
                self.announce(MeetingChangeKind::Started, &meeting).await;
                self.broadcast(ServerNotification::CoreStateChanged(
                    CoreStateChangedParams {
                        state: CoreState::Recording,
                    },
                ))
                .await;
                Ok(serde_json::to_value(MeetingResponse { meeting })?)
            }

            ClientRequest::MeetingStop(_) => {
                // Diarization is queued inside `stop_meeting`, where every
                // path that stops a Meeting converges, and worked by its own
                // task. Stopping must return at once — the Operator pressed
                // a button — and a model that fails or takes four minutes
                // must not be able to make stopping fail or feel slow.
                let meeting = self.core.stop_meeting().await?;
                self.announce(MeetingChangeKind::Stopped, &meeting).await;
                self.broadcast(ServerNotification::CoreStateChanged(
                    CoreStateChangedParams {
                        state: CoreState::Idle,
                    },
                ))
                .await;
                Ok(serde_json::to_value(MeetingResponse { meeting })?)
            }

            ClientRequest::MeetingList(params) => {
                let meetings = self
                    .core
                    .list_meetings(
                        params.limit.unwrap_or(DEFAULT_LIST_LIMIT),
                        params.offset.unwrap_or(0),
                    )
                    .await?;
                Ok(serde_json::to_value(MeetingListResponse { meetings })?)
            }

            ClientRequest::MeetingGet(params) => {
                let (meeting, segments) = self
                    .core
                    .get_meeting(&params.id)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("no Meeting with id {}", params.id))?;
                Ok(serde_json::to_value(MeetingDetailResponse {
                    meeting,
                    segments,
                })?)
            }

            ClientRequest::MeetingRetitle(params) => {
                let meeting = self.core.retitle_meeting(&params.id, &params.title).await?;
                self.announce(MeetingChangeKind::Updated, &meeting).await;
                Ok(serde_json::to_value(MeetingResponse { meeting })?)
            }

            ClientRequest::MeetingDelete(params) => {
                let deleted = self.core.delete_meeting(&params.id).await?;
                if deleted {
                    self.broadcast(ServerNotification::MeetingChanged(MeetingChangedParams {
                        kind: MeetingChangeKind::Deleted,
                        meeting_id: params.id.clone(),
                        meeting: None,
                    }))
                    .await;
                }
                Ok(serde_json::to_value(MeetingDeleteResponse { deleted })?)
            }

            ClientRequest::MeetingExport(params) => {
                let (markdown, mirror_path) = self
                    .core
                    .export_meeting(&params.id)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("no Meeting with id {}", params.id))?;
                Ok(serde_json::to_value(MeetingExportResponse {
                    markdown,
                    mirror_path,
                })?)
            }

            ClientRequest::HistorySearch(params) => {
                let results = self
                    .core
                    .search_history(&params.query, params.limit.unwrap_or(DEFAULT_SEARCH_LIMIT))
                    .await?;
                Ok(serde_json::to_value(HistorySearchResponse { results })?)
            }

            ClientRequest::ModelsStatus(_) => Ok(serde_json::to_value(self.core.models_status()?)?),

            ClientRequest::ModelsCancel(_) => {
                self.core.cancel_fetch();
                Ok(serde_json::to_value(self.core.models_status()?)?)
            }

            ClientRequest::TranscriptSubscribe(params) => {
                // Subscribing and snapshotting in one step is the point: a
                // Client that fetched then subscribed would lose any segment
                // completing between the two calls.
                if let Some(connection) = self.connections.get_mut(&connection_id) {
                    connection.captions = true;
                    connection.captions_dropped = 0;
                }
                let target = match params.meeting_id {
                    Some(id) => self.core.get_meeting(&id).await?,
                    None => self.core.current_meeting_with_transcript().await?,
                };
                let (meeting, segments) = match target {
                    Some((meeting, segments)) => (Some(meeting), segments),
                    None => (None, Vec::new()),
                };
                Ok(serde_json::to_value(TranscriptSnapshotResponse {
                    meeting,
                    segments,
                    subscribed: true,
                })?)
            }

            ClientRequest::SettingsGet(_) => Ok(serde_json::to_value(self.core.settings().await)?),

            ClientRequest::SettingsSet(params) => Ok(serde_json::to_value(
                self.core.update_settings(params).await?,
            )?),

            ClientRequest::WatchlistGet(_) => {
                Ok(serde_json::to_value(self.core.watchlist().await?)?)
            }

            ClientRequest::WatchlistAdd(params) => Ok(serde_json::to_value(
                self.core.watchlist_add(params).await?,
            )?),

            ClientRequest::WatchlistRemove(params) => Ok(serde_json::to_value(
                self.core.watchlist_remove(&params.id).await?,
            )?),

            ClientRequest::MeetingSetNotes(params) => {
                let meeting = self.core.set_notes(&params.id, &params.notes).await?;
                self.announce(MeetingChangeKind::Updated, &meeting).await;
                Ok(serde_json::to_value(MeetingResponse { meeting })?)
            }

            ClientRequest::SummaryBackends(_) => {
                Ok(serde_json::to_value(self.core.summary_backends().await)?)
            }

            ClientRequest::SummarySetKey(params) => Ok(serde_json::to_value(
                self.core
                    .set_summary_key(&params.provider, params.key.as_deref())
                    .await?,
            )?),

            ClientRequest::BriefingGet(params) => {
                let language = match params.language.as_deref() {
                    Some("zh") | Some("zh-CN") => {
                        crate::briefing::BriefingLanguage::SimplifiedChinese
                    }
                    _ => crate::briefing::BriefingLanguage::English,
                };
                Ok(serde_json::to_value(BriefingResponse {
                    text: crate::briefing::briefing(language),
                    acknowledged: self.core.briefing_acknowledged().await,
                    // No version has been reviewed by counsel. The PRD makes
                    // that review mandatory before v1; until it happens the
                    // product says so rather than implying otherwise.
                    awaiting_counsel: true,
                })?)
            }

            ClientRequest::PostureGet(_) => Ok(serde_json::to_value(self.core.posture().await?)?),

            ClientRequest::SpeakerList(_) => Ok(serde_json::to_value(self.core.speakers().await?)?),

            ClientRequest::SpeakerGet(params) => {
                Ok(serde_json::to_value(self.core.speaker(&params.id).await?)?)
            }

            ClientRequest::SpeakerRename(params) => Ok(serde_json::to_value(
                self.core
                    .speaker_rename(
                        &params.id,
                        &params.display_name,
                        params.join.unwrap_or(false),
                    )
                    .await?,
            )?),

            ClientRequest::SpeakerDeleteVoiceprint(params) => Ok(serde_json::to_value(
                self.core.speaker_delete_voiceprint(&params.id).await?,
            )?),

            ClientRequest::SpeakerSample(params) => Ok(serde_json::to_value(
                self.core.speaker_sample(&params.id).await?,
            )?),

            ClientRequest::TranscriptReassign(params) => Ok(serde_json::to_value(
                self.core
                    .reassign_segment(&params.segment_id, &params.speaker_id)
                    .await?,
            )?),

            ClientRequest::DiarizeStatus(_) => {
                Ok(serde_json::to_value(self.core.diarize_status().await?)?)
            }

            ClientRequest::DiarizeRun(params) => {
                let meeting_id = self
                    .core
                    .resolve_meeting(&params.meeting_id)
                    .await?
                    .unwrap_or_else(|| params.meeting_id.clone());
                // Told, not logged. M3 refused inside the spawned task and
                // wrote a line nobody reads, so the caller saw a status
                // saying a run had started when none had.
                if !self
                    .core
                    .enqueue_diarization(&meeting_id, diarize_queue::Priority::Front)
                    .await?
                {
                    anyhow::bail!(
                        "{meeting_id} is already being diarized or waiting its turn; \
                         `diarize status` says where it is in line"
                    );
                }
                Ok(serde_json::to_value(self.core.diarize_status().await?)?)
            }

            ClientRequest::DiarizeCancel(params) => Ok(serde_json::to_value(
                self.core.diarize_cancel(&params.meeting_id).await?,
            )?),

            ClientRequest::DiarizeRerunCancel(_) => Ok(serde_json::to_value(
                self.core.diarize_rerun_cancel().await?,
            )?),

            ClientRequest::TranscriptUnsubscribe(_) => {
                if let Some(connection) = self.connections.get_mut(&connection_id) {
                    connection.captions = false;
                }
                Ok(serde_json::to_value(TranscriptUnsubscribeResponse {
                    subscribed: false,
                })?)
            }

            // `dispatch_request` answers these off the loop, never here.
            ClientRequest::ModelsFetch(_)
            | ClientRequest::AudioCheck(_)
            | ClientRequest::SummaryGenerate(_)
            | ClientRequest::CalendarRequestAccess(_) => {
                anyhow::bail!("this request is answered off the server loop")
            }
        }
    }

    /// Delivers captions to subscribers, lossily.
    ///
    /// A subscriber whose queue is full loses this caption and is told how
    /// many it has missed. It is never disconnected and capture is never
    /// slowed — a slow UI must not be able to damage the recording
    /// (ADR-0028's deviation from codex, which disconnects slow clients).
    async fn broadcast_captions(&mut self, notification: ServerNotification) {
        let (method, params) = notification.to_wire();
        let message = JsonRpcMessage::Notification(evertranscript_protocol::JsonRpcNotification {
            method: method.to_string(),
            params: Some(params),
        });

        let mut catch_up = Vec::new();
        for (connection_id, connection) in self.connections.iter_mut() {
            if !connection.initialized || !connection.captions {
                continue;
            }
            if connection.writer.try_send(message.clone()).is_err() {
                connection.captions_dropped += 1;
                catch_up.push((*connection_id, connection.captions_dropped));
            } else if connection.captions_dropped > 0 {
                // Room again: tell them what they missed, then reset.
                catch_up.push((*connection_id, connection.captions_dropped));
                connection.captions_dropped = 0;
            }
        }

        for (connection_id, dropped) in catch_up {
            let Some(connection) = self.connections.get(&connection_id) else {
                continue;
            };
            if connection.captions_dropped > 0 {
                // Still behind; the gap notice can wait until there is room.
                continue;
            }
            let notice =
                ServerNotification::TranscriptCaptionsDropped(TranscriptCaptionsDroppedParams {
                    meeting_id: String::new(),
                    dropped,
                });
            let (method, params) = notice.to_wire();
            let _ = connection.writer.try_send(JsonRpcMessage::Notification(
                evertranscript_protocol::JsonRpcNotification {
                    method: method.to_string(),
                    params: Some(params),
                },
            ));
        }
    }

    fn handle_initialize(
        &mut self,
        connection_id: ConnectionId,
        params: InitializeParams,
    ) -> serde_json::Value {
        // Experimental methods are opt-in per connection, so a stable Client
        // never sees an unstable surface (ADR-0028).
        let experimental_api = params.capabilities.experimental_api;
        if let Some(connection) = self.connections.get_mut(&connection_id) {
            connection.initialized = true;
            connection.experimental_api = experimental_api;
        }
        debug!(
            connection_id,
            client = %params.client_info.name,
            version = %params.client_info.version,
            "client initialized"
        );
        serde_json::to_value(InitializeResponse {
            server_info: ServerInfo {
                name: evertranscript_protocol::SERVER_NAME.to_string(),
                version: evertranscript_protocol::VERSION.to_string(),
                protocol_version: evertranscript_protocol::PROTOCOL_VERSION,
            },
            capabilities: ServerCapabilities { experimental_api },
        })
        .unwrap_or(serde_json::Value::Null)
    }

    async fn announce(&mut self, kind: MeetingChangeKind, meeting: &Meeting) {
        self.broadcast(ServerNotification::MeetingChanged(MeetingChangedParams {
            kind,
            meeting_id: meeting.id.clone(),
            meeting: Some(meeting.clone()),
        }))
        .await;
    }

    async fn send(&mut self, connection_id: ConnectionId, message: JsonRpcMessage) {
        let Some(connection) = self.connections.get(&connection_id) else {
            return;
        };
        if connection.writer.send(message).await.is_err() {
            self.connections.remove(&connection_id);
        }
    }

    /// Pushes a notification to every initialized connection. Connections
    /// whose queue is full or closed are dropped rather than blocking the
    /// Core — capture must never wait on a Client.
    pub async fn broadcast(&mut self, notification: ServerNotification) {
        let (method, params) = notification.to_wire();
        let message = JsonRpcMessage::Notification(evertranscript_protocol::JsonRpcNotification {
            method: method.to_string(),
            params: Some(params),
        });
        let mut dead = Vec::new();
        for (connection_id, connection) in &self.connections {
            if !connection.initialized {
                continue;
            }
            if connection.writer.try_send(message.clone()).is_err() {
                dead.push(*connection_id);
            }
        }
        for connection_id in dead {
            debug!(connection_id, "dropping a client that fell behind");
            self.connections.remove(&connection_id);
        }
    }

    /// Sets Core state and tells everyone attached.
    pub async fn set_state(&mut self, state: CoreState) {
        self.core.set_state(state).await;
        self.broadcast(ServerNotification::CoreStateChanged(
            CoreStateChangedParams { state },
        ))
        .await;
    }
}

/// The Watchlist as a Client sees it.
fn describe_watchlist(list: &crate::detect::watchlist::Watchlist) -> WatchlistResponse {
    fn row(
        entry: &crate::detect::watchlist::WatchlistEntry,
    ) -> evertranscript_protocol::WatchlistEntry {
        evertranscript_protocol::WatchlistEntry {
            id: entry.id.clone(),
            name: entry.name.clone(),
            kind: match entry.kind {
                crate::detect::watchlist::EntryKind::Process => WatchlistKind::Process,
                crate::detect::watchlist::EntryKind::BrowserMeetings => {
                    WatchlistKind::BrowserMeetings
                }
            },
        }
    }
    WatchlistResponse {
        entries: list.entries().iter().map(row).collect(),
        suggestions: list.suggestions().iter().map(row).collect(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_stored_audio_path_uses_forward_slashes_on_every_platform() {
        // **ADR-0035: the History folder is the complete portable unit.** It
        // gets copied to another machine, synced, handed over. `Path::display`
        // writes the host's separator, so a Meeting recorded on Windows stored
        // `.data\audio\x.mp3` — resolvable on Windows and nowhere else.
        //
        // This went unnoticed because the failure is one-directional: Windows
        // accepts forward slashes, so every macOS-written record opened there
        // correctly, and only the reverse was broken. Windows CI caught it the
        // first time it got far enough to run `capture_vertical`.
        let history = std::path::Path::new("/tmp/History");
        let audio = history.join(".data").join("audio").join("01a074b1.mp3");
        let stored = Core::history_relative(history, &audio);

        assert_eq!(stored, ".data/audio/01a074b1.mp3");
        assert!(
            !stored.contains('\\'),
            "a separator the other platform cannot read: {stored}"
        );
        // And it stays relative, so moving the folder does not break it.
        assert!(!stored.starts_with('/'), "{stored}");
    }

    use super::*;

    const STARTED: &str = "2026-09-01T18:08:17.381177-07:00";

    #[test]
    fn a_recovered_orphan_is_dated_by_its_audio_not_its_untouched_row() {
        // The real one this came from: a Meeting whose Core died three days
        // earlier, whose row was never written again because there was no
        // transcription model to write segments, and whose recovered audio
        // ran for 69 hours. Dating it by `updated_at` made it a Meeting of
        // zero length holding 6 GB of sound.
        let bytes = 69 * 60 * 60 * audio::sink::BYTES_PER_SECOND;
        let ended = interrupted_end(STARTED, Some(STARTED.to_string()), Some(bytes));
        let start = chrono::DateTime::parse_from_rfc3339(STARTED).expect("start");
        let end = chrono::DateTime::parse_from_rfc3339(&ended).expect("end");
        assert_eq!((end - start).num_hours(), 69);
    }

    #[test]
    fn a_row_written_past_the_audio_keeps_its_own_evidence() {
        // Transcription outliving the audio that reached disk is evidence the
        // Meeting ran longer than the audio proves. Taking the audio here
        // would be the same mistake pointed the other way.
        let bytes = 60 * audio::sink::BYTES_PER_SECOND; // one minute of audio
        let touched = "2026-09-01T19:08:17.381177-07:00"; // an hour later
        let ended = interrupted_end(STARTED, Some(touched.to_string()), Some(bytes));
        assert_eq!(
            chrono::DateTime::parse_from_rfc3339(&ended).expect("end"),
            chrono::DateTime::parse_from_rfc3339(touched).expect("touched")
        );
    }

    /// A Meeting with words in it and a synthetic successful run over them.
    ///
    /// Enough for the completion half to have something real to write, so that
    /// "nothing was written" is a property of the stop rather than of an empty
    /// Meeting.
    async fn diarizable(core: &Arc<Core>) -> (String, impl Fn(Option<Prepared>) -> RunResult) {
        use crate::diarize::Diarization;
        use crate::diarize::Embedding;
        use crate::diarize::Turn;
        use evertranscript_protocol::AudioChannel;

        let meeting = core
            .store
            .write(|connection| {
                connection.execute(
                    "INSERT INTO meetings (id, started_at, created_at, updated_at, audio_path)
                     VALUES ('m1', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z',
                             '2026-01-01T00:00:00Z', 'm1.wav')",
                    [],
                )?;
                connection.execute(
                    "INSERT INTO transcript_segments (id, meeting_id, sequence, channel,
                     start_ms, end_ms, text)
                     VALUES ('s1', 'm1', 0, 'mic', 0, 11000, 'hello')",
                    [],
                )?;
                Ok("m1".to_string())
            })
            .await
            .expect("meeting");

        // Longer than `cluster::MIN_SPEAKER_MS`, or `persist` refuses to mint a
        // Speaker and the control would write nothing for a reason that has
        // nothing to do with any stop.
        let turns = vec![Turn::new(AudioChannel::Mic, 0, 12_000, 0)];
        let succeeded = move |prepared: Option<Prepared>| -> RunResult {
            Ok(Ran {
                result: Ok(Diarization {
                    turns: turns.clone(),
                    embeddings: [(
                        turns[0].cluster,
                        Embedding::new(
                            vec![1.0, 0.0],
                            crate::diarize::live::EMBEDDING_MODEL,
                            crate::diarize::live::EMBEDDING_MODEL_VERSION,
                            12_000,
                        ),
                    )]
                    .into_iter()
                    .collect(),
                }),
                rebuilt: Vec::new(),
                prepared: prepared.map(Ok),
            })
        };
        (meeting, succeeded)
    }

    /// Attributions, Speakers and the diarized mark: what a written run leaves.
    fn evidence(connection: &rusqlite::Connection) -> Result<(i64, i64, i64)> {
        Ok((
            connection.query_row(
                "SELECT count(*) FROM transcript_segments WHERE speaker_id IS NOT NULL",
                [],
                |row| row.get(0),
            )?,
            connection.query_row("SELECT count(*) FROM speakers", [], |row| row.get(0))?,
            connection.query_row(
                "SELECT count(*) FROM meetings WHERE diarized_at IS NOT NULL",
                [],
                |row| row.get(0),
            )?,
        ))
    }

    /// A stop that arrives after the run succeeded must still write nothing.
    ///
    /// `LiveDiarizer::observe` polls the token at window starts, and `diarize`
    /// returns `Ok` after its final progress tick — so a recording that starts
    /// during the last window, during clustering, or during that tick arrives
    /// with a finished `Diarization` already in hand. Nothing inside the run
    /// can refuse it by then.
    ///
    /// Needs no models, because the completion half takes the run's result as
    /// an argument. It tests completion only: `spawn_blocking` cleanup is not
    /// exercised by a direct call, and is covered by the `_slot` move and
    /// `runner::run_guarded`'s own tests instead.
    #[tokio::test]
    async fn a_stop_that_arrives_after_a_successful_pass_writes_nothing_and_stays_owed() {
        use crate::diarize::Cancel;
        use crate::store::diarize_queue::Priority;
        use std::sync::atomic::AtomicBool;
        use std::sync::atomic::Ordering::SeqCst;

        let dir = tempfile::tempdir().expect("tempdir");
        let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");
        let (meeting, succeeded) = diarizable(&core).await;
        core.enqueue_diarization(&meeting, Priority::Back)
            .await
            .expect("queue");

        // 1. Stood down by a recording that then ended before completion ran.
        // Reading live recording state here would call it an Operator
        // cancellation and lose the one fact the queue needs.
        let stop = Cancel::new();
        stop.cancel();
        let latched = Arc::new(AtomicBool::new(true));
        core.recording.store(false, SeqCst);
        assert_eq!(
            core.finish_run(&meeting, succeeded(None), &stop, &latched, None)
                .await
                .expect("finish"),
            DiarizeOutcome::Paused,
            "a recording stopped it, whether or not one is still running now"
        );
        assert_eq!(
            core.store.read(evidence).await.expect("read"),
            (0, 0, 0),
            "no attribution, no Speaker and no diarized mark"
        );

        // 2. A recording first seen *after* the run returned: nothing has
        // cancelled anything yet, and completion is what has to notice. The
        // cause is latched by `stand_down_for_recording` rather than supplied.
        let unset = Cancel::new();
        let unlatched = Arc::new(AtomicBool::new(false));
        core.recording.store(true, SeqCst);
        assert_eq!(
            core.finish_run(
                &meeting,
                succeeded(None),
                &unset,
                &unlatched,
                Some(Arc::clone(&core.recording)),
            )
            .await
            .expect("finish"),
            DiarizeOutcome::Paused
        );
        assert!(
            unlatched.load(SeqCst) && unset.is_cancelled(),
            "and the cause was written down where a later reader can find it"
        );
        assert_eq!(core.store.read(evidence).await.expect("read"), (0, 0, 0));
        assert!(
            core.diarization_holds(&meeting).await.expect("holds"),
            "still owed after both"
        );

        // 3. A stop with no recording behind it is the Operator's, and
        // `diarize/cancel` has already taken the row out.
        core.recording.store(false, SeqCst);
        let cancelled = Cancel::new();
        cancelled.cancel();
        assert_eq!(
            core.finish_run(
                &meeting,
                succeeded(None),
                &cancelled,
                &Arc::new(AtomicBool::new(false)),
                Some(Arc::clone(&core.recording)),
            )
            .await
            .expect("finish"),
            DiarizeOutcome::Cancelled
        );
        assert_eq!(
            core.store.read(evidence).await.expect("read"),
            (0, 0, 0),
            "an Operator's stop writes nothing either"
        );

        // 4. The resumed path, and the control without which every assertion
        // above passes on a `finish_run` that never writes anything at all.
        // Same result, same eligibility, nothing recording.
        let outcome = core
            .finish_run(
                &meeting,
                succeeded(None),
                &Cancel::new(),
                &Arc::new(AtomicBool::new(false)),
                Some(Arc::clone(&core.recording)),
            )
            .await
            .expect("finish");
        assert!(
            matches!(outcome, DiarizeOutcome::Wrote(attributed) if attributed > 0),
            "once nothing is recording the same run writes: {outcome:?}"
        );
        let (attributed, speakers, diarized) = core.store.read(evidence).await.expect("read");
        assert!(attributed > 0 && speakers > 0 && diarized > 0);
    }

    /// A recording that starts while the write is still queued stops it too.
    ///
    /// `Store::write` hands the closure to one writer thread and waits, so a
    /// check on the calling side is a check before an unbounded wait. Held
    /// here with a barrier rather than a sleep: an earlier closure reports that
    /// it is running and then blocks the writer, completion is polled once —
    /// far enough to queue its own closure behind that one, and no further,
    /// with nothing recording at the time — and only then does the recording
    /// start and the writer get released.
    ///
    /// That single poll is what makes the test worth having. A check moved back
    /// to the calling side would run during it, see nothing recording, and
    /// write.
    #[tokio::test]
    async fn a_recording_that_starts_while_the_write_waits_stops_it_before_the_transaction() {
        use crate::diarize::Cancel;
        use std::future::Future;
        use std::sync::atomic::AtomicBool;
        use std::sync::atomic::Ordering::SeqCst;
        use std::task::Poll;

        let dir = tempfile::tempdir().expect("tempdir");
        let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");
        let (meeting, succeeded) = diarizable(&core).await;

        // The writer is a thread of its own, so blocking it is what a barrier
        // is for; the arrival signal comes back the other way and has to be
        // awaited, because this test's runtime is the one that would otherwise
        // be blocked waiting for it.
        let (arrived, arrival) = tokio::sync::oneshot::channel::<()>();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let store = core.store.clone();
        let hold = tokio::spawn(async move {
            store
                .write(move |_| {
                    arrived.send(()).expect("report");
                    released.recv().expect("wait to be released");
                    Ok(())
                })
                .await
        });
        arrival.await.expect("the writer is occupied");

        let stop = Cancel::new();
        let latched = Arc::new(AtomicBool::new(false));
        let mut completing = Box::pin(core.finish_run(
            &meeting,
            succeeded(None),
            &stop,
            &latched,
            Some(Arc::clone(&core.recording)),
        ));
        let queued =
            std::future::poll_fn(|context| Poll::Ready(completing.as_mut().poll(context))).await;
        assert!(
            queued.is_pending(),
            "completion got as far as the writer's queue, with nothing recording yet"
        );

        core.recording.store(true, SeqCst);
        release.send(()).expect("release the writer");
        hold.await.expect("join").expect("held write");

        assert_eq!(
            completing.await.expect("finish"),
            DiarizeOutcome::Paused,
            "the writer found a recording waiting for it"
        );
        assert!(
            latched.load(SeqCst),
            "with the cause recorded on the writer thread"
        );
        assert_eq!(
            core.store.read(evidence).await.expect("read"),
            (0, 0, 0),
            "and wrote nothing"
        );
    }

    /// Installs ticket 12's tables, which no migration does.
    ///
    /// Explicit here for the same reason it is explicit in
    /// `store::rerun`'s own tests: the pending SQL is unregistered, so a test
    /// that reached these paths without asking for them would be testing a
    /// History no installation has.
    async fn with_rerun_tables(core: &Arc<Core>) {
        core.store
            .write(|connection| {
                connection
                    .execute_batch(crate::store::schema::PENDING_MODEL_CHANGE_RERUN)
                    .map_err(Into::into)
            })
            .await
            .expect("the pending re-run tables");
    }

    /// Meetings with Kept Audio, so a re-run has something to enqueue.
    async fn meetings_with_audio(core: &Arc<Core>, ids: &[&str]) {
        let ids: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
        core.store
            .write(move |connection| {
                for (nth, id) in ids.iter().enumerate() {
                    connection.execute(
                        "INSERT INTO meetings (id, started_at, created_at, updated_at, audio_path)
                         VALUES (?1, ?2, ?2, ?2, ?3)",
                        rusqlite::params![
                            id,
                            format!("2024-01-0{}T00:00:00Z", nth + 1),
                            format!("{id}.wav")
                        ],
                    )?;
                }
                Ok(())
            })
            .await
            .expect("history");
    }

    async fn begin_rerun(core: &Arc<Core>) -> usize {
        core.store
            .write(|connection| crate::store::rerun::begin(connection, "redimnet2-b3", "1"))
            .await
            .expect("begin")
    }

    use crate::diarize::Cancel;
    use rusqlite::OptionalExtension;
    use std::sync::atomic::AtomicBool;

    /// The whole-cluster vector the run produces. Deliberately unlike any
    /// bounded vector below, so that a substitution is visible rather than
    /// having to be argued about.
    const WHOLE_CLUSTER: [f32; 2] = [0.5, 0.866];
    /// What the ranges attributed to Alice embed to.
    const ALICE_BOUNDED: [f32; 2] = [0.0, 1.0];
    /// And Bob's, pointing the other way so a resolve cannot confuse them.
    const BOB_BOUNDED: [f32; 2] = [-1.0, 0.0];

    /// A Meeting the bulk re-run owns, with named Speakers attributed in it
    /// by the previous model, and the plan the worker would have read before
    /// inference.
    ///
    /// `mixed` puts a second named Speaker's segment in the same cluster, so
    /// `claims` finds no unanimous owner. That is the case the ranges have to
    /// survive on their own: a named Speaker's own audio is still theirs when
    /// the cluster around it comes out holding somebody else too.
    async fn reseedable(core: &Arc<Core>, mixed: bool) -> (String, crate::diarize::reseed::Plan) {
        core.store
            .write(move |connection| {
                connection.execute(
                    "INSERT INTO meetings (id, started_at, created_at, updated_at, audio_path)
                     VALUES ('m1', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z',
                             '2026-01-01T00:00:00Z', 'm1.wav')",
                    [],
                )?;
                for (id, name) in [("alice", "Alice"), ("bob", "Bob")] {
                    connection.execute(
                        "INSERT INTO speakers (id, display_name, confirmed, voiceprint,
                         voiceprint_model, voiceprint_model_version, created_at)
                         VALUES (?1, ?2, 1, X'0000803F', 'old', '1', 'now')",
                        rusqlite::params![id, name],
                    )?;
                }
                // The previous model's evidence, in its own space. Re-seeding
                // replaces exactly this.
                connection.execute(
                    "INSERT INTO speaker_exemplars
                     (id, speaker_id, meeting_id, embedding, model, model_version,
                      voiced_ms, source, created_at)
                     VALUES ('old1', 'alice', 'm1', X'0000803F', 'old', '1', 1000,
                             'machine', 'now')",
                    [],
                )?;
                connection.execute(
                    "INSERT INTO transcript_segments (id, meeting_id, sequence, channel,
                     start_ms, end_ms, text, speaker_id)
                     VALUES ('s1', 'm1', 0, 'mic', 0, 11000, 'hello', 'alice')",
                    [],
                )?;
                if mixed {
                    connection.execute(
                        "INSERT INTO transcript_segments (id, meeting_id, sequence, channel,
                         start_ms, end_ms, text, speaker_id)
                         VALUES ('s2', 'm1', 1, 'mic', 11000, 12000, 'hi', 'bob')",
                        [],
                    )?;
                }
                Ok(())
            })
            .await
            .expect("history");
        with_rerun_tables(core).await;
        assert_eq!(begin_rerun(core).await, 1, "the backlog owns this Meeting");

        let plan = core
            .store
            .read(|connection| crate::diarize::reseed::plan(connection, "m1"))
            .await
            .expect("plan")
            .expect("kept audio");
        ("m1".to_string(), plan)
    }

    /// The vectors the model would have produced for that plan, in order.
    fn bounded_for(plan: &crate::diarize::reseed::Plan) -> Vec<Option<Vec<f32>>> {
        plan.ranges
            .iter()
            .map(|range| {
                Some(match range.speaker_id.as_str() {
                    "alice" => ALICE_BOUNDED.to_vec(),
                    _ => BOB_BOUNDED.to_vec(),
                })
            })
            .collect()
    }

    /// A run over that Meeting whose one cluster carries `WHOLE_CLUSTER`.
    fn run_over(plan: Option<crate::diarize::reseed::Plan>, vector: [f32; 2]) -> RunResult {
        use crate::diarize::Diarization;
        use crate::diarize::Embedding;
        use crate::diarize::Turn;
        use evertranscript_protocol::AudioChannel;

        let turns = vec![Turn::new(AudioChannel::Mic, 0, 12_000, 0)];
        let prepared = plan.map(|plan| {
            let vectors = bounded_for(&plan);
            Ok(Prepared { plan, vectors })
        });
        Ok(Ran {
            result: Ok(Diarization {
                turns: turns.clone(),
                embeddings: [(
                    turns[0].cluster,
                    Embedding::new(
                        vector.to_vec(),
                        crate::diarize::live::EMBEDDING_MODEL,
                        crate::diarize::live::EMBEDDING_MODEL_VERSION,
                        12_000,
                    ),
                )]
                .into_iter()
                .collect(),
            }),
            rebuilt: Vec::new(),
            prepared,
        })
    }

    /// Every exemplar a Speaker holds for this Meeting, as vectors.
    async fn exemplars_of(core: &Arc<Core>, speaker_id: &str) -> Vec<Vec<f32>> {
        let speaker_id = speaker_id.to_string();
        core.store
            .read(move |connection| {
                Ok(crate::store::speakers::exemplars(connection, &speaker_id)?
                    .into_iter()
                    .map(|exemplar| exemplar.vector)
                    .collect())
            })
            .await
            .expect("exemplars")
    }

    /// Whether any Speaker anywhere holds the whole-cluster vector.
    async fn anyone_holds_the_cluster_vector(core: &Arc<Core>) -> bool {
        core.store
            .read(|connection| {
                let mut statement =
                    connection.prepare("SELECT embedding FROM speaker_exemplars")?;
                let rows = statement.query_map([], |row| row.get::<_, Vec<u8>>(0))?;
                let mut found = false;
                for row in rows {
                    let bytes = row?;
                    let vector: Vec<f32> = bytes
                        .chunks_exact(4)
                        .map(|four| f32::from_le_bytes([four[0], four[1], four[2], four[3]]))
                        .collect();
                    if vector
                        .iter()
                        .zip(WHOLE_CLUSTER.iter())
                        .all(|(a, b)| (a - b).abs() < 1e-6)
                        && vector.len() == WHOLE_CLUSTER.len()
                    {
                        found = true;
                    }
                }
                Ok(found)
            })
            .await
            .expect("scan")
    }

    async fn attributed_to(core: &Arc<Core>, segment_id: &str) -> Option<String> {
        let segment_id = segment_id.to_string();
        core.store
            .read(move |connection| {
                connection
                    .query_row(
                        "SELECT speaker_id FROM transcript_segments WHERE id = ?1",
                        rusqlite::params![segment_id],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .map_err(Into::into)
            })
            .await
            .expect("attribution")
    }

    async fn still_queued(core: &Arc<Core>, meeting_id: &str) -> bool {
        let meeting_id = meeting_id.to_string();
        core.store
            .read(move |connection| {
                Ok(connection
                    .query_row(
                        "SELECT 1 FROM diarize_queue WHERE meeting_id = ?1",
                        rusqlite::params![meeting_id],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some())
            })
            .await
            .expect("queue")
    }

    /// **Bounded evidence has to survive the step that installs centroids.**
    ///
    /// `persist_with` deletes this Meeting's machine exemplars for every
    /// Speaker it resolves and files the whole cluster's vector instead. That
    /// vector is built over every grouped observation, so it carries audio no
    /// transcript segment covers — and filing it under a name the Operator
    /// trusts is exactly what re-seeding exists to avoid. Here the cluster is
    /// mixed, so `claims` abstains and only the ranges protect Alice.
    #[tokio::test]
    async fn rebuilt_ranges_survive_the_run_that_would_have_replaced_them() {
        let dir = tempfile::tempdir().expect("tempdir");
        let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");
        let (meeting, plan) = reseedable(&core, true).await;
        assert_eq!(plan.ranges.len(), 2, "one range each for Alice and Bob");

        core.finish_run(
            &meeting,
            run_over(Some(plan), WHOLE_CLUSTER),
            &Cancel::new(),
            &Arc::new(AtomicBool::new(false)),
            None,
        )
        .await
        .expect("finish");

        assert_eq!(
            exemplars_of(&core, "alice").await,
            vec![ALICE_BOUNDED.to_vec()],
            "Alice keeps the vector cut from her own ranges, and only that"
        );
        assert!(
            !anyone_holds_the_cluster_vector(&core).await,
            "and the whole-cluster vector was filed under nobody"
        );
    }

    /// A claim settles who the words belong to. It settles nothing about the
    /// vector.
    ///
    /// The cluster here resolves nowhere near Alice — without the claim this
    /// run would mint a pseudonym and enrol the whole-cluster vector under
    /// it, and Alice would lose the words she already owned.
    #[tokio::test]
    async fn an_unanimous_claim_attributes_without_enrolling_the_cluster() {
        let dir = tempfile::tempdir().expect("tempdir");
        let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");
        let (meeting, plan) = reseedable(&core, false).await;

        core.finish_run(
            &meeting,
            // Orthogonal to Alice's rebuilt Voiceprint, so nothing about this
            // attribution can have come from the resolve.
            run_over(Some(plan), [1.0, 0.0]),
            &Cancel::new(),
            &Arc::new(AtomicBool::new(false)),
            None,
        )
        .await
        .expect("finish");

        assert_eq!(
            attributed_to(&core, "s1").await.as_deref(),
            Some("alice"),
            "the Operator's standing word decided this, not the vectors"
        );
        assert_eq!(
            exemplars_of(&core, "alice").await,
            vec![ALICE_BOUNDED.to_vec()],
            "and the cluster she was given did not join her evidence"
        );
        let speakers: i64 = core
            .store
            .read(|connection| {
                connection
                    .query_row("SELECT count(*) FROM speakers", [], |row| row.get(0))
                    .map_err(Into::into)
            })
            .await
            .expect("count");
        assert_eq!(
            speakers, 2,
            "and no pseudonym was minted for a claimed voice"
        );
    }

    /// A correction landing while the model ran leaves the vectors describing
    /// a world that has moved. The Meeting is owed a fresh pass, not walked.
    #[tokio::test]
    async fn a_correction_during_inference_keeps_the_meeting_owed_and_writes_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");
        let (meeting, plan) = reseedable(&core, false).await;

        // The Operator moves the segment while inference is running.
        core.store
            .write(|connection| {
                connection.execute(
                    "UPDATE transcript_segments SET speaker_id = 'bob' WHERE id = 's1'",
                    [],
                )?;
                Ok(())
            })
            .await
            .expect("correction");

        let outcome = core
            .finish_run(
                &meeting,
                run_over(Some(plan), WHOLE_CLUSTER),
                &Cancel::new(),
                &Arc::new(AtomicBool::new(false)),
                None,
            )
            .await
            .expect("finish");

        assert_eq!(
            outcome,
            DiarizeOutcome::Owed,
            "still owed — not walked, and not a recording pause"
        );
        assert!(
            still_queued(&core, &meeting).await,
            "so the queue row is still there for the next pass"
        );
        assert_eq!(
            core.store.read(evidence).await.expect("read"),
            (1, 2, 0),
            "and nothing was attributed or marked diarized by this run"
        );
        assert_eq!(
            exemplars_of(&core, "alice").await.len(),
            1,
            "Alice still holds exactly the previous model's exemplar"
        );
    }

    /// Everything the re-run writes is in the attribution transaction, so a
    /// failure anywhere after it takes the re-seeding with it.
    ///
    /// The trigger makes `reconcile::apply` fail, which is the last write
    /// before the commit — after `reseed::commit` has already put Alice's
    /// rebuilt evidence in. Rolled back, she is left with what she had.
    #[tokio::test]
    async fn a_failure_after_reseeding_leaves_no_rebuilt_evidence_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");
        let (meeting, plan) = reseedable(&core, false).await;
        core.store
            .write(|connection| {
                connection.execute_batch(
                    "CREATE TRIGGER boom BEFORE UPDATE ON transcript_segments \
                     BEGIN SELECT RAISE(ABORT, 'boom'); END;",
                )?;
                Ok(())
            })
            .await
            .expect("trigger");

        let outcome = core
            .finish_run(
                &meeting,
                run_over(Some(plan), WHOLE_CLUSTER),
                &Cancel::new(),
                &Arc::new(AtomicBool::new(false)),
                None,
            )
            .await;

        assert!(outcome.is_err(), "the transaction failed: {outcome:?}");
        assert_eq!(
            exemplars_of(&core, "alice").await,
            vec![vec![1.0_f32]],
            "and Alice holds the previous model's exemplar, not a rebuilt one"
        );
        assert!(
            still_queued(&core, &meeting).await,
            "with the Meeting still in the line"
        );
    }

    /// A stop found waiting on the writer must leave the re-seeding undone
    /// too, not only the attribution.
    #[tokio::test]
    async fn a_stop_before_the_transaction_leaves_no_rebuilt_evidence_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");
        let (meeting, plan) = reseedable(&core, false).await;

        let stop = Cancel::new();
        stop.cancel();
        let outcome = core
            .finish_run(
                &meeting,
                run_over(Some(plan), WHOLE_CLUSTER),
                &stop,
                &Arc::new(AtomicBool::new(false)),
                None,
            )
            .await
            .expect("finish");

        assert_eq!(outcome, DiarizeOutcome::Cancelled);
        assert_eq!(
            exemplars_of(&core, "alice").await,
            vec![vec![1.0_f32]],
            "the previous model's exemplar, untouched"
        );
    }

    /// A History with no backlog serializes exactly what it always did.
    ///
    /// Two cases, and the second is the one worth guarding: an installation
    /// that has merely written down which embedding it is in has a re-run
    /// row, and reporting that as a re-run of zero Meetings would put a
    /// backlog in front of every Operator who never asked for one.
    #[tokio::test]
    async fn a_history_without_a_backlog_serializes_the_status_it_always_did() {
        let dir = tempfile::tempdir().expect("tempdir");
        let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");

        let before =
            serde_json::to_value(core.diarize_status().await.expect("status")).expect("json");
        assert!(
            before.get("rerun").is_none(),
            "no tables, so no key at all — not a null and not a zeroed block: {before}"
        );

        with_rerun_tables(&core).await;
        meetings_with_audio(&core, &["m1", "m2"]).await;
        core.store
            .write(|connection| {
                crate::store::rerun::begin_if_the_model_changed(connection, "wespeaker", "2")
            })
            .await
            .expect("first start");

        let baseline =
            serde_json::to_value(core.diarize_status().await.expect("status")).expect("json");
        assert!(
            baseline.get("rerun").is_none(),
            "a recorded model with no backlog behind it is not a re-run: {baseline}"
        );
        assert_eq!(baseline, before, "byte for byte the status it always was");
    }

    /// Walked, still owed and given up on are three different numbers.
    #[tokio::test]
    async fn the_rerun_block_tells_walked_owed_and_given_up_apart() {
        use std::sync::atomic::Ordering::SeqCst;

        let dir = tempfile::tempdir().expect("tempdir");
        let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");
        with_rerun_tables(&core).await;
        meetings_with_audio(&core, &["m1", "m2", "m3"]).await;
        assert_eq!(begin_rerun(&core).await, 3);

        let asked = core
            .diarize_status()
            .await
            .expect("status")
            .rerun
            .expect("a backlog");
        assert_eq!(
            (asked.total, asked.done, asked.remaining, asked.abandoned),
            (3, 0, 3, 0)
        );
        assert!(!asked.cancelled && !asked.paused_for_recording);

        // One walked, the ordinary way: the worker takes its row out.
        core.store
            .write(|connection| crate::store::diarize_queue::finish(connection, "m1"))
            .await
            .expect("walked");
        let partway = core
            .diarize_status()
            .await
            .expect("status")
            .rerun
            .expect("a backlog");
        assert_eq!((partway.total, partway.done, partway.remaining), (3, 1, 2));

        // A recording holds the rest. Still owed, so a wait and not a stop.
        core.recording.store(true, SeqCst);
        let paused = core
            .diarize_status()
            .await
            .expect("status")
            .rerun
            .expect("a backlog");
        assert!(
            paused.paused_for_recording && !paused.cancelled && paused.remaining == 2,
            "a recording pauses the backlog; it does not cancel it: {paused:?}"
        );
        core.recording.store(false, SeqCst);

        // Stopped. The two still in line were given up on, and the one
        // already walked stays walked.
        core.diarize_rerun_cancel().await.expect("stop");
        let stopped = core
            .diarize_status()
            .await
            .expect("status")
            .rerun
            .expect("a backlog");
        assert_eq!(
            (stopped.done, stopped.remaining, stopped.abandoned),
            (1, 0, 2),
            "one walked and two given up, not three walked: {stopped:?}"
        );
        assert!(stopped.cancelled && !stopped.paused_for_recording);
    }

    /// Cancelling the backlog is not cancelling the queue.
    ///
    /// Bulk stop, not [`Core::diarize_cancel`] applied widely: the catch-up
    /// pass for Meetings that were never diarized shares the `Back` class,
    /// and a Meeting somebody is waiting for is no longer this job's.
    #[tokio::test]
    async fn cancelling_the_rerun_leaves_front_and_unrelated_bulk_work_alone() {
        use crate::store::diarize_queue::Priority;

        let dir = tempfile::tempdir().expect("tempdir");
        let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");
        with_rerun_tables(&core).await;
        meetings_with_audio(&core, &["m1", "m2", "m3", "catchup"]).await;
        // `catchup` is enqueued before the backlog exists, so the re-run
        // never owns it — the same shape production's catch-up pass has.
        core.enqueue_diarization("catchup", Priority::Back)
            .await
            .expect("catch-up");
        assert_eq!(begin_rerun(&core).await, 3, "not the catch-up Meeting");
        core.enqueue_diarization("m3", Priority::Front)
            .await
            .expect("asked for");

        // And it is the one being walked. Somebody is waiting for it, so a
        // bulk stop is not its to give: the token must survive.
        let promoted = crate::diarize::Cancel::new();
        *core.diarization.lock().await = Some(DiarizeJob {
            meeting_id: "m3".to_string(),
            cancel: promoted.clone(),
            done_ms: 0,
            total_ms: 1,
        });

        core.diarize_rerun_cancel().await.expect("stop");
        assert!(
            !promoted.is_cancelled(),
            "a promoted Meeting is no longer this backlog's to stop"
        );
        *core.diarization.lock().await = None;

        assert_eq!(
            core.diarize_status().await.expect("status").queued,
            vec!["m3".to_string(), "catchup".to_string()],
            "what somebody is waiting for, and what the re-run never asked for"
        );
        let stopped = core
            .diarize_status()
            .await
            .expect("status")
            .rerun
            .expect("a backlog");
        assert_eq!(
            (stopped.done, stopped.remaining, stopped.abandoned),
            (0, 1, 2),
            "the promoted Meeting is still owed, and promotion is not completion"
        );
    }

    /// A promotion committing *while* the stop runs still saves the job.
    ///
    /// The fixture above promotes before the stop begins, which an eligibility
    /// read taken outside the mutation would also pass. This is the
    /// interleaving that tells them apart, and it is built rather than timed:
    /// the writer is occupied by a closure that performs the promotion itself
    /// just before it returns, so the promotion is guaranteed to commit after
    /// the stop has started and before the stop's own transaction runs. An
    /// eligibility answer read on entry is stale by then; one taken inside the
    /// transaction is not.
    #[tokio::test]
    async fn a_promotion_that_commits_while_the_stop_waits_still_saves_the_job() {
        let dir = tempfile::tempdir().expect("tempdir");
        let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");
        with_rerun_tables(&core).await;
        meetings_with_audio(&core, &["m1", "m2"]).await;
        assert_eq!(begin_rerun(&core).await, 2);

        let running = crate::diarize::Cancel::new();
        *core.diarization.lock().await = Some(DiarizeJob {
            meeting_id: "m1".to_string(),
            cancel: running.clone(),
            done_ms: 0,
            total_ms: 1,
        });

        let (arrived, arrival) = tokio::sync::oneshot::channel::<()>();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let store = core.store.clone();
        let hold = tokio::spawn(async move {
            store
                .write(move |connection| {
                    arrived.send(()).expect("report");
                    released.recv().expect("wait to be released");
                    // Somebody asks for the running Meeting, on the writer
                    // thread, with the stop's own closure already waiting
                    // behind this one.
                    crate::store::diarize_queue::enqueue(
                        connection,
                        "m1",
                        crate::store::diarize_queue::Priority::Front,
                    )
                })
                .await
        });
        arrival.await.expect("the writer is occupied");

        let stopping = tokio::spawn({
            let core = Arc::clone(&core);
            async move { core.diarize_rerun_cancel().await }
        });
        // One yield is enough on this runtime to carry the spawned stop to
        // its first pending await, which is the writer's answer — so its
        // closure is queued behind the held one before the promotion lands.
        tokio::task::yield_now().await;

        release.send(()).expect("release the writer");
        hold.await.expect("join").expect("held write");
        stopping.await.expect("join").expect("stop");

        assert!(
            !running.is_cancelled(),
            "the promotion committed before the stop's transaction, so that \
             Meeting was somebody's wait by the time the stop decided"
        );
        assert!(
            core.diarize_status()
                .await
                .expect("status")
                .queued
                .contains(&"m1".to_string()),
            "and it is still in the line"
        );
        let stopped = core
            .diarize_status()
            .await
            .expect("status")
            .rerun
            .expect("a backlog");
        assert_eq!(
            (stopped.remaining, stopped.abandoned),
            (1, 1),
            "promotion is not completion: still owed, and not given up on"
        );
    }

    /// A Meeting stopped before its run registers never starts.
    ///
    /// The worker reads the head of the queue and then calls in. A bulk stop
    /// landing in that window used to remove the row and count the Meeting
    /// abandoned while the run went ahead and committed new evidence after
    /// the stop. Registration now revalidates under the same lock.
    #[tokio::test]
    async fn a_meeting_stopped_before_its_run_registers_does_not_run() {
        use crate::store::diarize_queue::Priority;

        let dir = tempfile::tempdir().expect("tempdir");
        let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");
        with_rerun_tables(&core).await;
        let (meeting, _succeeded) = diarizable(&core).await;
        assert_eq!(begin_rerun(&core).await, 1);

        // Enough for the checks ahead of registration to pass. Empty files:
        // the point is that this returns before anything opens them, so a
        // refusal here cannot be the models being absent.
        let models = core.models_dir.clone();
        std::fs::create_dir_all(&models).expect("models dir");
        for name in ["diarize-segmentation.onnx", "diarize-embedding.onnx"] {
            std::fs::write(models.join(name), []).expect("placeholder");
        }
        std::fs::write(core.history_dir.join("m1.wav"), []).expect("audio");

        // The stop has already happened — the state the worker is in when it
        // is holding a name it read a moment ago.
        core.diarize_rerun_cancel().await.expect("stop");

        assert_eq!(
            core.diarize_meeting(&meeting, Priority::Back)
                .await
                .expect("run"),
            DiarizeOutcome::Cancelled,
            "the row is gone, so there is nothing to run"
        );
        assert_eq!(
            core.store.read(evidence).await.expect("read"),
            (0, 0, 0),
            "and nothing was written after the stop"
        );
        assert!(
            core.diarization.lock().await.is_none(),
            "nor was a job claimed"
        );
    }

    /// Stopping the backlog stops the Meeting it is walking right now.
    #[tokio::test]
    async fn a_backlog_stopped_mid_meeting_stops_that_meeting_without_writing() {
        use crate::diarize::Cancel;
        use std::sync::atomic::AtomicBool;

        let dir = tempfile::tempdir().expect("tempdir");
        let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");
        with_rerun_tables(&core).await;
        let (meeting, succeeded) = diarizable(&core).await;
        meetings_with_audio(&core, &["m2"]).await;
        assert_eq!(begin_rerun(&core).await, 2);

        // The worker as it is while a run is in flight: the row is still in
        // the queue, because `peek` does not consume and the run takes it out
        // only when it commits.
        let cancel = Cancel::new();
        *core.diarization.lock().await = Some(DiarizeJob {
            meeting_id: meeting.clone(),
            cancel: cancel.clone(),
            done_ms: 0,
            total_ms: 1,
        });

        core.diarize_rerun_cancel().await.expect("stop");
        assert!(
            cancel.is_cancelled(),
            "the running Meeting was this backlog's bulk work"
        );

        // And the run, which had already succeeded by then, writes nothing.
        *core.diarization.lock().await = None;
        assert_eq!(
            core.finish_run(
                &meeting,
                succeeded(None),
                &cancel,
                &Arc::new(AtomicBool::new(false)),
                None,
            )
            .await
            .expect("finish"),
            DiarizeOutcome::Cancelled
        );
        assert_eq!(
            core.store.read(evidence).await.expect("read"),
            (0, 0, 0),
            "nothing attributed, no Speaker, no diarized mark"
        );
    }

    /// A run that committed leaves the line inside its own transaction.
    ///
    /// Which is what makes a later stop unable to miscount it: there is no
    /// window in which a walked Meeting still looks owed, so nothing has to
    /// infer from a wall-clock stamp which run committed. Two stamps a
    /// ten-thousandth of a second apart round to the same `julianday`, and
    /// the clock can be adjusted under them; neither is a completion
    /// identity.
    #[tokio::test]
    async fn a_committed_run_leaves_the_line_in_the_same_transaction() {
        use crate::diarize::Cancel;
        use std::sync::atomic::AtomicBool;

        let dir = tempfile::tempdir().expect("tempdir");
        let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");
        with_rerun_tables(&core).await;
        let (meeting, succeeded) = diarizable(&core).await;
        meetings_with_audio(&core, &["m2", "m3"]).await;
        assert_eq!(begin_rerun(&core).await, 3);

        // The counterexample the old discriminator failed on: a Meeting
        // walked *before* this backlog began, whose stamp is indistinguishable
        // from the backlog's own start.
        core.store
            .write(|connection| {
                connection.execute(
                    "UPDATE meetings SET diarized_at = '2026-09-16T17:00:00.100100-07:00'
                      WHERE id = 'm2'",
                    [],
                )?;
                connection.execute(
                    "UPDATE diarize_rerun SET started_at = '2026-09-16T17:00:00.100200-07:00'
                      WHERE id = 1",
                    [],
                )?;
                Ok(())
            })
            .await
            .expect("stamps");

        let written = core
            .finish_run(
                &meeting,
                succeeded(None),
                &Cancel::new(),
                &Arc::new(AtomicBool::new(false)),
                None,
            )
            .await
            .expect("finish");
        assert!(matches!(written, DiarizeOutcome::Wrote(n) if n > 0));
        assert!(
            !core.diarization_holds(&meeting).await.expect("holds"),
            "the commit took its own row out; there is no gap to race"
        );

        core.diarize_rerun_cancel().await.expect("stop");
        let stopped = core
            .diarize_status()
            .await
            .expect("status")
            .rerun
            .expect("a backlog");
        assert_eq!(
            (stopped.done, stopped.remaining, stopped.abandoned),
            (1, 0, 2),
            "one walked and two given up — and `m2`, whose old stamp reads as \
             later than the backlog's start, is among the two: {stopped:?}"
        );
    }

    /// Stopping what was never started is not a stop.
    ///
    /// `begin_if_the_model_changed` writes a row on first start to record
    /// which embedding the History is in. Setting `cancelled` on that would
    /// manufacture a stopped re-run out of metadata and put one in front of
    /// an Operator who never asked for anything.
    #[tokio::test]
    async fn stopping_a_backlog_that_was_never_asked_for_changes_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");

        // No tables at all.
        let before =
            serde_json::to_value(core.diarize_status().await.expect("status")).expect("json");
        let after =
            serde_json::to_value(core.diarize_rerun_cancel().await.expect("stop")).expect("json");
        assert_eq!(after, before, "nothing to stop, and nothing said about it");
        assert!(after.get("rerun").is_none());

        // Tables, and the identity the first start records.
        with_rerun_tables(&core).await;
        meetings_with_audio(&core, &["m1"]).await;
        core.store
            .write(|connection| {
                crate::store::rerun::begin_if_the_model_changed(connection, "wespeaker", "2")
            })
            .await
            .expect("first start");

        let baseline =
            serde_json::to_value(core.diarize_status().await.expect("status")).expect("json");
        let stopped =
            serde_json::to_value(core.diarize_rerun_cancel().await.expect("stop")).expect("json");
        assert_eq!(stopped, baseline);
        assert!(
            stopped.get("rerun").is_none(),
            "a recorded model is not a backlog, stopped or otherwise: {stopped}"
        );
        assert!(
            !core
                .store
                .read(crate::store::rerun::state)
                .await
                .expect("state")
                .expect("a row")
                .cancelled,
            "and the row it did record was left alone"
        );
    }

    /// Runs the worker until the queue drains, or gives up.
    ///
    /// Bounded: a loop that never drains is the failure being tested, so the
    /// deadline is short and the queue it was still holding is returned for
    /// the assertion to name.
    async fn drain(core: &Arc<Core>, within: std::time::Duration) -> Vec<String> {
        let shutdown = tokio_util::sync::CancellationToken::new();
        let worker = tokio::spawn(Arc::clone(core).run_diarization_queue(shutdown.clone()));
        let deadline = std::time::Instant::now() + within;
        let mut queued = core.diarize_status().await.expect("status").queued;
        while !queued.is_empty() && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            queued = core.diarize_status().await.expect("status").queued;
        }
        shutdown.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), worker)
            .await
            .expect("the worker did not stop when shutdown was cancelled")
            .expect("the worker panicked");
        queued
    }

    /// A run that fails before any transaction still leaves the line.
    ///
    /// It has to. Only a committed run takes its own row out, so a failure
    /// that stayed reported as written would leave the row at the head, and
    /// the loop would read it, fail the same way and never reach the work
    /// behind it — a spin, not a retry.
    #[tokio::test]
    async fn a_run_that_fails_before_writing_leaves_the_line_and_the_queue_moves_on() {
        use crate::store::diarize_queue::Priority;

        let dir = tempfile::tempdir().expect("tempdir");
        let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");
        meetings_with_audio(&core, &["gone", "next"]).await;
        core.enqueue_diarization("gone", Priority::Back)
            .await
            .expect("queue");
        core.enqueue_diarization("next", Priority::Back)
            .await
            .expect("queue");

        // A queue row whose Meeting is not there. `diarize_meeting` bails on
        // it, which is the shape of every failure that happens before the
        // run: the outcome is an `Err`, not an outcome. Written with the key
        // check off, because the cascade exists to stop exactly this — the
        // point is the arm that handles it, not how a row got there.
        core.store
            .write(|connection| {
                connection.pragma_update(None, "foreign_keys", false)?;
                connection.execute("DELETE FROM meetings WHERE id = 'gone'", [])?;
                connection.pragma_update(None, "foreign_keys", true)?;
                Ok(())
            })
            .await
            .expect("orphan");
        assert_eq!(
            core.diarize_status().await.expect("status").queued,
            vec!["gone".to_string(), "next".to_string()],
            "the premise: the failing one is at the head"
        );

        assert!(
            drain(&core, std::time::Duration::from_secs(2))
                .await
                .is_empty(),
            "the failure left the line, and the Meeting behind it was reached"
        );
    }

    /// Cancelling one Meeting of the backlog is giving up on it, not walking it.
    ///
    /// The queue removal alone made the arithmetic lie: the row left, the
    /// membership stayed, `remaining` fell and `done` — total minus remaining
    /// minus abandoned — counted a Meeting nobody had walked.
    #[tokio::test]
    async fn cancelling_one_of_the_backlogs_meetings_counts_it_given_up_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");
        with_rerun_tables(&core).await;
        meetings_with_audio(&core, &["m1", "m2", "m3"]).await;
        assert_eq!(begin_rerun(&core).await, 3);

        core.diarize_cancel("m2").await.expect("cancel");
        let after = core
            .diarize_status()
            .await
            .expect("status")
            .rerun
            .expect("a backlog");
        assert_eq!(
            (after.done, after.remaining, after.abandoned),
            (0, 2, 1),
            "given up on, not walked: {after:?}"
        );
        assert!(!after.cancelled, "the backlog itself is still running");

        // Again, and it must still be one. The queue row is the gate.
        core.diarize_cancel("m2").await.expect("cancel again");
        assert_eq!(
            core.diarize_status()
                .await
                .expect("status")
                .rerun
                .expect("a backlog")
                .abandoned,
            1,
            "cancelling twice is one Meeting given up on"
        );
    }

    /// A Meeting the backlog never owned, and one whose run already committed.
    ///
    /// Neither may move `abandoned`: the first was never the re-run's, and the
    /// second took its own queue row out inside the transaction that wrote its
    /// attribution, so there is nothing left to give up.
    #[tokio::test]
    async fn cancelling_unowned_or_already_committed_work_does_not_count_it_given_up() {
        use crate::diarize::Cancel;
        use crate::store::diarize_queue::Priority;
        use std::sync::atomic::AtomicBool;

        let dir = tempfile::tempdir().expect("tempdir");
        let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");
        with_rerun_tables(&core).await;
        let (meeting, succeeded) = diarizable(&core).await;
        meetings_with_audio(&core, &["m2"]).await;
        assert_eq!(begin_rerun(&core).await, 2);
        // Recorded *after* the backlog began, so it is nobody's but the
        // queue's — `begin` enqueues the History it finds, and this was not
        // in it.
        meetings_with_audio(&core, &["catchup"]).await;
        core.enqueue_diarization("catchup", Priority::Front)
            .await
            .expect("catch-up");

        core.diarize_cancel("catchup").await.expect("cancel");
        assert_eq!(
            core.diarize_status()
                .await
                .expect("status")
                .rerun
                .expect("a backlog")
                .abandoned,
            0,
            "work the re-run never asked for is not work it gave up on"
        );

        core.finish_run(
            &meeting,
            succeeded(None),
            &Cancel::new(),
            &Arc::new(AtomicBool::new(false)),
            None,
        )
        .await
        .expect("finish");
        core.diarize_cancel(&meeting).await.expect("cancel");
        let after = core
            .diarize_status()
            .await
            .expect("status")
            .rerun
            .expect("a backlog");
        assert_eq!(
            (after.done, after.remaining, after.abandoned),
            (1, 1, 0),
            "it was walked before the cancel arrived, and stays walked: {after:?}"
        );
    }

    /// Cancelling on an installation with no backlog does not invent one.
    #[tokio::test]
    async fn cancelling_one_meeting_never_manufactures_a_backlog() {
        use crate::store::diarize_queue::Priority;

        let dir = tempfile::tempdir().expect("tempdir");
        let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");
        meetings_with_audio(&core, &["m1"]).await;
        core.enqueue_diarization("m1", Priority::Front)
            .await
            .expect("queue");

        let cancelled = core.diarize_cancel("m1").await.expect("cancel");
        assert!(
            cancelled.rerun.is_none() && cancelled.queued.is_empty(),
            "out of the line, and no re-run said to exist: {cancelled:?}"
        );

        // And with the tables there but only the first start's identity in
        // them, which is metadata rather than a backlog.
        with_rerun_tables(&core).await;
        core.store
            .write(|connection| {
                crate::store::rerun::begin_if_the_model_changed(connection, "wespeaker", "2")
            })
            .await
            .expect("first start");
        core.enqueue_diarization("m1", Priority::Front)
            .await
            .expect("queue");
        assert!(
            core.diarize_cancel("m1")
                .await
                .expect("cancel")
                .rerun
                .is_none(),
            "a recorded model is still not a backlog"
        );
    }

    /// A run that starts after a cancel has finished finds nothing to claim.
    ///
    /// The half of the ordering a test can pin deterministically: cancel runs
    /// to completion, *then* the run starts, and registration's re-read of
    /// the queue turns it away before it claims anything. The other half —
    /// that the token check and the removal are one hold of the job lock, so
    /// a run arriving mid-cancel either registers first and is found, or
    /// arrives after and finds no row — is a property of the lock's lifetime
    /// and rests on reading `diarize_cancel`, not on this test.
    #[tokio::test]
    async fn a_run_registering_after_a_cancel_finds_the_work_already_gone() {
        use crate::store::diarize_queue::Priority;

        let dir = tempfile::tempdir().expect("tempdir");
        let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");
        let (meeting, _succeeded) = diarizable(&core).await;

        // Past the checks that precede registration; nothing opens them.
        let models = core.models_dir.clone();
        std::fs::create_dir_all(&models).expect("models dir");
        for name in ["diarize-segmentation.onnx", "diarize-embedding.onnx"] {
            std::fs::write(models.join(name), []).expect("placeholder");
        }
        std::fs::write(core.history_dir.join("m1.wav"), []).expect("audio");

        core.diarize_cancel(&meeting).await.expect("cancel");
        assert_eq!(
            core.diarize_meeting(&meeting, Priority::Front)
                .await
                .expect("run"),
            DiarizeOutcome::Cancelled,
            "the row is gone, so the run stops before claiming anything"
        );
        assert_eq!(
            core.store.read(evidence).await.expect("read"),
            (0, 0, 0),
            "and nothing was written after the Operator was told it stopped"
        );
        assert!(core.diarization.lock().await.is_none());
    }

    /// Which outcomes still owe the queue a removal, and which must not.
    ///
    /// The `Wrote` row is the one with teeth: that run took its own row out
    /// inside the commit, so a second removal afterwards would fall on
    /// whatever row is under that id now — which can be a request somebody
    /// made in the meantime.
    #[test]
    fn only_a_run_that_never_reached_a_transaction_is_taken_out_afterwards() {
        for (outcome, afterwards, why) in [
            (
                DiarizeOutcome::Skipped,
                true,
                "nothing ran, so nothing removed it",
            ),
            (
                DiarizeOutcome::Wrote(0),
                false,
                "the commit removed its own row",
            ),
            (
                DiarizeOutcome::Wrote(7),
                false,
                "the commit removed its own row",
            ),
            (DiarizeOutcome::Paused, false, "still owed on purpose"),
            (
                DiarizeOutcome::Cancelled,
                false,
                "the cancellation took it out",
            ),
        ] {
            assert_eq!(leaves_the_line_afterwards(outcome), afterwards, "{why}");
        }
    }

    /// A request made after a run commits survives the worker.
    ///
    /// The window the rule above protects: the commit takes its own row out,
    /// and between that and the worker's next step somebody asks for the same
    /// Meeting again. A removal there would drop a request nobody cancelled.
    #[tokio::test]
    async fn a_request_made_after_a_run_commits_is_not_swept_up_by_it() {
        use crate::diarize::Cancel;
        use crate::store::diarize_queue::Priority;
        use std::sync::atomic::AtomicBool;

        let dir = tempfile::tempdir().expect("tempdir");
        let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");
        let (meeting, succeeded) = diarizable(&core).await;
        core.enqueue_diarization(&meeting, Priority::Back)
            .await
            .expect("queue");

        let outcome = core
            .finish_run(
                &meeting,
                succeeded(None),
                &Cancel::new(),
                &Arc::new(AtomicBool::new(false)),
                None,
            )
            .await
            .expect("finish");
        assert!(matches!(outcome, DiarizeOutcome::Wrote(_)));
        assert!(
            !core.diarization_holds(&meeting).await.expect("holds"),
            "the commit took its own row out"
        );

        // Somebody asks again — a re-run of a Meeting they just watched
        // finish, which is the ordinary way this happens.
        core.enqueue_diarization(&meeting, Priority::Front)
            .await
            .expect("asked again");
        assert!(
            !leaves_the_line_afterwards(outcome),
            "so the worker must not remove anything, or it removes this"
        );
        assert!(
            core.diarization_holds(&meeting).await.expect("holds"),
            "and the new request is still owed"
        );
    }

    /// The one rule the queue worker's pause is: only bulk work yields.
    ///
    /// Stated here rather than only in the worker because the live version
    /// needs the ONNX models to reach, and this is the decision itself —
    /// every other part of the pause is what the worker does with the answer.
    #[test]
    fn only_bulk_work_stands_down_for_a_recording() {
        use crate::store::diarize_queue::Priority;

        assert!(
            yields_to_recording(Priority::Back, true),
            "an overnight re-run does not take the machine off a call"
        );
        assert!(
            !yields_to_recording(Priority::Front, true),
            "and a Meeting somebody is waiting for keeps its turn regardless"
        );
        assert!(!yields_to_recording(Priority::Back, false));
        assert!(!yields_to_recording(Priority::Front, false));
    }

    #[test]
    fn with_no_audio_and_no_writes_it_claims_nothing() {
        // Nothing survived and nothing ever touched the row, so there is no
        // evidence of any duration. Inventing one is the whole failure.
        assert_eq!(interrupted_end(STARTED, None, None), STARTED);
    }

    #[test]
    fn an_unparseable_start_falls_back_rather_than_guessing() {
        assert_eq!(
            interrupted_end("not a timestamp", Some("also not".to_string()), Some(4096)),
            "also not"
        );
    }

    fn history() -> rusqlite::Connection {
        let mut connection = rusqlite::Connection::open_in_memory().expect("open");
        crate::store::schema::migrate(&mut connection).expect("migrate");
        connection
    }

    #[test]
    fn re_running_after_a_deleted_voiceprint_does_not_mint_a_second_you() {
        // The defect, end to end. Delete the Operator's Voiceprint and
        // re-run one Meeting: the cluster that used to be recognized is a
        // stranger now and gets a fresh Speaker, and the old code set the
        // flag on *that* — leaving two flagged rows, with the lookup still
        // returning the first. A Registry with two "You" in it.
        //
        // Now the flag is an address, not a label: it stays on the row it is
        // on, and the run's voices are folded into it.
        use crate::diarize::Cluster;
        use crate::diarize::operator::Identified;
        use crate::store::speakers;

        let connection = history();
        let you = speakers::create(&connection, true).expect("the Operator");
        let minted = speakers::create(&connection, false).expect("this run's stranger");

        let assigned = std::collections::BTreeMap::from([(Cluster(0), minted.id.clone())]);
        let attached = Core::attach_operator(
            &connection,
            &Identified::Dominant(Cluster(0)),
            &assigned,
            "m",
        )
        .expect("attach");

        assert_eq!(attached.as_deref(), Some(you.id.as_str()));
        assert_eq!(
            speakers::list(&connection)
                .expect("list")
                .iter()
                .filter(|speaker| speaker.is_operator)
                .count(),
            1,
            "one 'You', which is the whole point"
        );
        assert!(
            speakers::get(&connection, &minted.id)
                .expect("get")
                .is_none(),
            "and the freshly minted row was folded in rather than left beside it"
        );
    }

    #[test]
    fn an_isolated_mic_does_not_swallow_a_colleague_history_already_knows() {
        // Rule 1 names *every* mic-channel voice. In a room where one of
        // them is someone History knows by name, that is a contradiction,
        // and deleting the name to resolve it would be the worst answer
        // available — so the name wins and the fold is skipped.
        use crate::diarize::Cluster;
        use crate::diarize::operator::Identified;
        use crate::store::speakers;

        let connection = history();
        let mine = speakers::create(&connection, false).expect("a voice of mine");
        let alice = speakers::create(&connection, false).expect("alice");
        speakers::rename(&connection, &alice.id, "Alice").expect("rename");

        let assigned = std::collections::BTreeMap::from([
            (Cluster(0), mine.id.clone()),
            (Cluster(1), alice.id.clone()),
        ]);
        let attached = Core::attach_operator(
            &connection,
            &Identified::IsolatedMic(vec![Cluster(0), Cluster(1)]),
            &assigned,
            "m",
        )
        .expect("attach");

        assert_eq!(attached.as_deref(), Some(mine.id.as_str()));
        assert_eq!(
            speakers::get(&connection, &alice.id)
                .expect("get")
                .expect("still there")
                .display_name
                .as_deref(),
            Some("Alice"),
            "a named Speaker is never folded away"
        );
    }

    #[test]
    fn identifying_nobody_leaves_the_flag_exactly_where_it_was() {
        use crate::diarize::operator::Identified;
        use crate::store::speakers;

        let connection = history();
        let you = speakers::create(&connection, true).expect("the Operator");
        let attached = Core::attach_operator(
            &connection,
            &Identified::Nobody,
            &std::collections::BTreeMap::new(),
            "m",
        )
        .expect("attach");

        assert_eq!(attached, None, "no rule fired, so nothing is claimed");
        assert_eq!(
            speakers::operator(&connection)
                .expect("lookup")
                .map(|speaker| speaker.id),
            Some(you.id),
            "and a Meeting that named nobody does not un-name the Operator"
        );
    }
}
