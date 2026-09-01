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
use evertranscript_protocol::ClientNotification;
use evertranscript_protocol::ClientRequest;
use evertranscript_protocol::CoreState;
use evertranscript_protocol::CoreStateChangedParams;
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
use evertranscript_protocol::SpeakerListResponse;
use evertranscript_protocol::SpeakerResponse;
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
    /// The recording in progress, if any. The Meeting owns it; capture
    /// streams inside it come and go (ADR-0029 as amended).
    recorder: Mutex<Option<audio::recorder::Recorder>>,
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

/// A stored Speaker as the protocol shows it, with its appearance counts.
///
/// The counts are derived here rather than stored on the row, so they cannot
/// drift from the segments they describe.
fn speaker_to_wire(
    connection: &rusqlite::Connection,
    row: crate::store::speakers::Speaker,
) -> Result<Speaker> {
    let (meetings_seen_in, first_seen_at) =
        crate::store::speakers::appearances(connection, &row.id)?;
    Ok(Speaker {
        id: row.id,
        display_name: row.display_name,
        is_operator: row.is_operator,
        has_voiceprint: row.has_voiceprint,
        confirmed: row.confirmed,
        voiceprint_model: row.voiceprint_model,
        meetings_seen_in,
        first_seen_at,
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
            recorder: Mutex::new(None),
            source_factory: Mutex::new(live_source_factory()),
            notifications: broadcast::channel(NOTIFICATION_CAPACITY).0,
            transcriber_factory: Mutex::new(None),
            summary_backend_factory: std::sync::Mutex::new(None),
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
        let id = id.to_string();
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

        let local = || -> Option<Box<dyn summary::Backend + 'static>> {
            let model = self
                .models_dir
                .join(crate::models::registry::SUMMARY_DEFAULT.filename);
            let binary = summarizer_binary()?;
            // How this model wants to be driven, from the registry rather
            // than from constants in the sidecar.
            let driving = crate::models::registry::SUMMARY_DEFAULT
                .driving
                .as_ref()
                .map(summary::sidecar::Driving::from_entry);
            summary::sidecar::SidecarBackend::spawn_driven(
                &binary,
                &model.to_string_lossy(),
                driving,
            )
            .ok()
            .map(|backend| Box::new(backend) as Box<dyn summary::Backend + 'static>)
        };

        let choice = settings
            .summary_backend
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("no Summary Backend has been chosen"))?;

        if choice == "local" {
            let backend = local()
                .ok_or_else(|| anyhow::anyhow!("the local Summary model is not available"))?;
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
        Ok((Box::new(chosen), local()))
    }

    /// Generates a Summary for a finished Meeting.
    pub async fn summarize_meeting(&self, meeting_id: &str) -> Result<String> {
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
            let mut parts = vec![summary::prompt::scrub(&first.text)];
            let mut failed = 0usize;
            for piece in &chunks[1..] {
                if cancel.is_cancelled() {
                    anyhow::bail!("{}", summary::BackendError::Cancelled);
                }
                match winner.generate(&request_for(piece), &cancel) {
                    Ok(text) => parts.push(summary::prompt::scrub(&text)),
                    Err(summary::BackendError::Cancelled) => {
                        anyhow::bail!("{}", summary::BackendError::Cancelled)
                    }
                    Err(_) => failed += 1,
                }
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
                let reduce = request_for(&format!(
                    "These are summaries of consecutive parts of one meeting. \
                     Combine them into a single summary in the same format.\n\n{combined}"
                ));
                match winner.generate(&reduce, &cancel) {
                    Ok(text) => summary::prompt::scrub(&text),
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
            })
        })
        .await??;

        // Already scrubbed per part inside the run.
        let markdown = outcome.text.clone();
        let used = outcome.used.label();
        // **What the Summary lost, in the record rather than only the log.**
        // A Summary assembled from five chunks of six is a different thing
        // from a complete one, and the Operator cannot read the Core's log.
        let gaps = (outcome.failed_chunks > 0).then(|| {
            tracing::warn!(
                failed = outcome.failed_chunks,
                of = outcome.chunks,
                "some chunks of this Meeting could not be summarized"
            );
            format!(
                "{} of {} parts of this meeting could not be summarized, \
                 so this Summary does not cover all of it.",
                outcome.failed_chunks, outcome.chunks
            )
        });
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

        let id = meeting_id.to_string();
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
        let id = id.to_string();
        self.store
            .read(move |connection| {
                let row = crate::store::speakers::get(connection, &id)?
                    .ok_or_else(|| anyhow::anyhow!("no Speaker with id {id}"))?;
                let speaker = speaker_to_wire(connection, row)?;
                let name_suggestions = crate::store::speakers::name_suggestions(connection, &id)?;
                Ok(SpeakerDetailResponse {
                    speaker,
                    name_suggestions,
                })
            })
            .await
    }

    /// Names a Speaker, which also confirms its Voiceprint.
    pub async fn speaker_rename(&self, id: &str, display_name: &str) -> Result<SpeakerResponse> {
        let id = id.to_string();
        let display_name = display_name.to_string();
        let speaker = self
            .store
            .write(move |connection| {
                let row = crate::store::speakers::rename(connection, &id, &display_name)?;
                speaker_to_wire(connection, row)
            })
            .await?;
        let _ = self
            .notifications
            .send(ServerNotification::SpeakerChanged(SpeakerChangedParams {
                speaker: speaker.clone(),
            }));
        self.mirror_wake.notify_one();
        Ok(SpeakerResponse { speaker })
    }

    /// Deletes a Speaker's Voiceprint (story 31). The record is untouched.
    pub async fn speaker_delete_voiceprint(&self, id: &str) -> Result<SpeakerResponse> {
        let id = id.to_string();
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
        Ok(SpeakerResponse { speaker })
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
    pub async fn diarize_meeting(&self, meeting_id: &str) -> Result<usize> {
        use crate::diarize;

        let Some(meeting) = self.get_meeting(meeting_id).await?.map(|(m, _)| m) else {
            anyhow::bail!("no Meeting with id {meeting_id}");
        };
        let Some(audio_path) = meeting.audio_path.clone() else {
            // A Meeting whose audio was never written, or was deleted. Not
            // an error: there is simply nothing to listen to.
            return Ok(0);
        };
        // Stored relative to the History folder so the record stays portable
        // (ADR-0035) — resolving it is the caller's job, not the row's.
        let audio_path = self.history_dir.join(audio_path);
        if !audio_path.exists() {
            tracing::info!(path = %audio_path.display(), "the Meeting's audio is gone; nothing to diarize");
            return Ok(0);
        }

        let segmentation = self.models_dir.join("diarize-segmentation.onnx");
        let embedding = self.models_dir.join("diarize-embedding.onnx");
        if !segmentation.exists() || !embedding.exists() {
            tracing::info!(
                "diarization models are not downloaded; leaving the Meeting unattributed"
            );
            return Ok(0);
        }

        let cancel = diarize::Cancel::new();
        *self.diarization.lock().await = Some(DiarizeJob {
            meeting_id: meeting_id.to_string(),
            cancel: cancel.clone(),
            done_ms: 0,
            total_ms: 0,
        });
        let notifications = self.notifications.clone();
        let id_for_progress = meeting_id.to_string();

        // The models are CPU-bound C++; keeping them off the async runtime is
        // what stops a long Meeting from stalling every Client request.
        let outcome = tokio::task::spawn_blocking(move || -> Result<_> {
            let _slot = diarize::runner::Slot::claim(&id_for_progress)
                .map_err(|busy| anyhow::anyhow!("{busy}"))?;
            let decoded = diarize::runner::decode(&audio_path)?;
            let mut diarizer = diarize::live::LiveDiarizer::load(&segmentation, &embedding)
                .map_err(|error| anyhow::anyhow!("{error}"))?;

            let mut last_percent = u64::MAX;
            let result = diarize::runner::run_guarded(
                &mut diarizer,
                decoded.audio(),
                &mut |progress| {
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
            Ok(result)
        })
        .await?;

        *self.diarization.lock().await = None;

        let diarization = match outcome {
            Ok(Ok(diarization)) => diarization,
            Ok(Err(diarize::DiarizeError::Cancelled)) => return Ok(0),
            Ok(Err(error)) => {
                tracing::warn!(%error, "diarization did not run; the Meeting is unattributed");
                return Ok(0);
            }
            Err(error) => {
                tracing::warn!(%error, "diarization failed; the Meeting is unattributed");
                return Ok(0);
            }
        };

        let meeting_id = meeting_id.to_string();
        let written = self
            .store
            .write(move |connection| {
                let assigned =
                    diarize::cluster::persist(connection, &meeting_id, &diarization.embeddings)?;

                // The Operator's own Speaker, where the evidence supports one
                // (ADR-0029 as amended).
                let known = diarize::operator::known_operator(connection)?;
                if let Some(mine) = diarize::operator::identify(&diarization, known.as_ref())
                    && let Some(speaker_id) = assigned.get(&mine)
                {
                    connection.execute(
                        "UPDATE speakers SET is_operator = 1 WHERE id = ?1",
                        rusqlite::params![speaker_id],
                    )?;
                }

                let segments = crate::store::meetings::segments(connection, &meeting_id)?;
                let reconciliation = diarize::reconcile::reconcile(&diarization, &segments);
                tracing::info!(
                    boundary_flips = reconciliation.boundary_flips,
                    attributed = reconciliation.attributed(),
                    "diarization reconciled"
                );
                diarize::reconcile::apply(
                    connection,
                    &reconciliation,
                    &assigned,
                    crate::store::speakers::Attribution::Clustered,
                )
            })
            .await?;

        self.mirror_wake.notify_one();
        Ok(written)
    }

    /// Starts Diarization for a finished Meeting without waiting for it.
    ///
    /// Detached on purpose. Attribution arriving minutes later is the design
    /// (ADR-0009's join exists because the Transcript is already published),
    /// and anything that made stopping wait for two neural models would make
    /// the one act the Operator performs by hand feel broken.
    pub fn diarize_in_background(self: std::sync::Arc<Self>, meeting_id: String) {
        tokio::spawn(async move {
            match self.diarize_meeting(&meeting_id).await {
                Ok(0) => {}
                Ok(attributed) => {
                    tracing::info!(meeting = %meeting_id, attributed, "Diarization attributed a Meeting")
                }
                // Never fatal, and never the Meeting's problem: the record
                // stands whether or not anyone could be identified in it.
                Err(error) => {
                    tracing::warn!(meeting = %meeting_id, %error, "Diarization did not complete")
                }
            }
        });
    }

    /// What Diarization is doing.
    pub async fn diarize_status(&self) -> DiarizeStatusResponse {
        match self.diarization.lock().await.as_ref() {
            Some(job) => DiarizeStatusResponse {
                state: DiarizeState::Running,
                meeting_id: Some(job.meeting_id.clone()),
                done_ms: job.done_ms as i64,
                total_ms: job.total_ms as i64,
            },
            None => DiarizeStatusResponse {
                state: DiarizeState::Idle,
                meeting_id: None,
                done_ms: 0,
                total_ms: 0,
            },
        }
    }

    /// Stops a running Diarization, keeping whatever attribution completed.
    pub async fn diarize_cancel(&self, meeting_id: &str) -> DiarizeStatusResponse {
        if let Some(job) = self.diarization.lock().await.as_ref()
            && job.meeting_id == meeting_id
        {
            job.cancel.cancel();
        }
        self.diarize_status().await
    }

    /// The Watchlist as Meeting Detection needs it.
    pub async fn watchlist_for_detection(&self) -> Result<crate::detect::watchlist::Watchlist> {
        self.store.read(crate::store::watchlist::load).await
    }

    /// Whether a Meeting is being recorded right now.
    pub async fn is_recording(&self) -> bool {
        self.recorder.lock().await.is_some()
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

    /// Merges any checkpoints a previous Core left behind after a crash.
    pub async fn recover_interrupted_audio(&self) {
        match audio::sink::recover_interrupted(&self.audio_dir()).await {
            Ok(recoveries) if !recoveries.is_empty() => {
                info!(
                    count = recoveries.len(),
                    "recovered audio from recordings a previous run did not finish"
                );
            }
            Ok(_) => {}
            Err(error) => warn!(%error, "audio recovery failed"),
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
            mirror::id8(&meeting.id),
            captions,
        ) {
            Ok(recorder) => {
                *self.recorder.lock().await = Some(recorder);
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
            let outcome = recorder.finish().await;
            if let Some(path) = &outcome.audio_path {
                let relative = self.relative_to_history(path);
                let id = meeting.id.clone();
                self.store
                    .write(move |connection| meetings::set_audio_path(connection, &id, &relative))
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
        self.get_meeting(&meeting.id)
            .await?
            .map(|(meeting, _)| meeting)
            .ok_or_else(|| anyhow::anyhow!("the Meeting vanished after stopping"))
    }

    /// Paths are stored relative to the History folder so the record stays
    /// portable: moving the folder must not break every audio reference
    /// (ADR-0035).
    fn relative_to_history(&self, path: &std::path::Path) -> String {
        path.strip_prefix(&self.history_dir)
            .unwrap_or(path)
            .display()
            .to_string()
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

    pub async fn get_meeting(&self, id: &str) -> Result<Option<(Meeting, Vec<TranscriptSegment>)>> {
        let id = id.to_string();
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
        let (id, title) = (id.to_string(), title.to_string());
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

    /// Downloads what is missing. A corrupted file is removed first so the
    /// fetch starts from a clean slate rather than trying to resume garbage.
    pub async fn fetch_models(&self, key: Option<&str>, cancel: CancellationToken) -> Result<()> {
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
            downloader
                .fetch(entry, cancel.clone(), |progress| {
                    debug!(
                        model = entry.key,
                        percent = (progress.fraction() * 100.0) as u32,
                        "downloading"
                    );
                })
                .await?;
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
                let response = self
                    .dispatch_request(
                        connection_id,
                        request.id.clone(),
                        &request.method,
                        request.params,
                    )
                    .await;
                self.send(connection_id, response).await;
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
    ) -> JsonRpcMessage {
        let request = match ClientRequest::from_wire(method, params) {
            Ok(request) => request,
            Err(evertranscript_protocol::DecodeError::UnknownMethod(method)) => {
                return JsonRpcMessage::Error(JsonRpcError::new(
                    id,
                    error_codes::METHOD_NOT_FOUND,
                    format!("unknown method: {method}"),
                ));
            }
            Err(err) => {
                return JsonRpcMessage::Error(JsonRpcError::new(
                    id,
                    error_codes::INVALID_PARAMS,
                    err.to_string(),
                ));
            }
        };

        let initialized = self
            .connections
            .get(&connection_id)
            .is_some_and(|connection| connection.initialized);

        match (&request, initialized) {
            (ClientRequest::Initialize(_), true) => {
                return JsonRpcMessage::Error(JsonRpcError::new(
                    id,
                    error_codes::ALREADY_INITIALIZED,
                    "this connection is already initialized",
                ));
            }
            (request, false) if !matches!(request, ClientRequest::Initialize(_)) => {
                return JsonRpcMessage::Error(JsonRpcError::new(
                    id,
                    error_codes::NOT_INITIALIZED,
                    "send initialize before any other request",
                ));
            }
            _ => {}
        }

        match self.handle(connection_id, request).await {
            Ok(result) => JsonRpcMessage::Response(JsonRpcResponse { id, result }),
            Err(error) => JsonRpcMessage::Error(JsonRpcError::new(
                id,
                error_codes::INTERNAL_ERROR,
                error.to_string(),
            )),
        }
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
                let meeting = self.core.stop_meeting().await?;
                // Diarization runs *after* the Meeting is safely persisted
                // and detached from this response. Stopping must return at
                // once — the Operator pressed a button — and a model that
                // fails or takes four minutes must not be able to make
                // stopping fail or feel slow.
                self.core.clone().diarize_in_background(meeting.id.clone());
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

            ClientRequest::ModelsFetch(params) => {
                self.core
                    .fetch_models(params.key.as_deref(), CancellationToken::new())
                    .await?;
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

            ClientRequest::SummaryGenerate(params) => {
                self.core.summarize_meeting(&params.id).await?;
                let meeting = self
                    .core
                    .get_meeting(&params.id)
                    .await?
                    .map(|(meeting, _)| meeting)
                    .ok_or_else(|| anyhow::anyhow!("the Meeting vanished"))?;
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
                    .speaker_rename(&params.id, &params.display_name)
                    .await?,
            )?),

            ClientRequest::SpeakerDeleteVoiceprint(params) => Ok(serde_json::to_value(
                self.core.speaker_delete_voiceprint(&params.id).await?,
            )?),

            ClientRequest::TranscriptReassign(params) => Ok(serde_json::to_value(
                self.core
                    .reassign_segment(&params.segment_id, &params.speaker_id)
                    .await?,
            )?),

            ClientRequest::DiarizeStatus(_) => {
                Ok(serde_json::to_value(self.core.diarize_status().await)?)
            }

            ClientRequest::DiarizeRun(params) => {
                self.core
                    .clone()
                    .diarize_in_background(params.meeting_id.clone());
                Ok(serde_json::to_value(self.core.diarize_status().await)?)
            }

            ClientRequest::DiarizeCancel(params) => Ok(serde_json::to_value(
                self.core.diarize_cancel(&params.meeting_id).await,
            )?),

            ClientRequest::TranscriptUnsubscribe(_) => {
                if let Some(connection) = self.connections.get_mut(&connection_id) {
                    connection.captions = false;
                }
                Ok(serde_json::to_value(TranscriptUnsubscribeResponse {
                    subscribed: false,
                })?)
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
