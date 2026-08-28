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
use evertranscript_protocol::ClientNotification;
use evertranscript_protocol::ClientRequest;
use evertranscript_protocol::CoreState;
use evertranscript_protocol::CoreStateChangedParams;
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
use evertranscript_protocol::RequestId;
use evertranscript_protocol::ServerCapabilities;
use evertranscript_protocol::ServerInfo;
use evertranscript_protocol::ServerNotification;
use evertranscript_protocol::SettingsResponse;
use evertranscript_protocol::SettingsSetParams;
use evertranscript_protocol::StatusResponse;
use evertranscript_protocol::TranscriptCaptionsDroppedParams;
use evertranscript_protocol::TranscriptSegment;
use evertranscript_protocol::TranscriptSegmentAddedParams;
use evertranscript_protocol::TranscriptSnapshotResponse;
use evertranscript_protocol::TranscriptUnsubscribeResponse;
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
            settings: Mutex::new(Settings::load_from(&settings_path)),
            settings_path,
            models_dir,
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
            settings.save_to(&self.settings_path)?;
        }
        Ok(self.settings().await)
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
                meetings::start(connection, title.as_deref(), detected_app.as_deref())
            })
            .await?;

        // Capture starts after the Meeting exists, so a recording can never
        // be running without a row to attach it to.
        let source = (self.source_factory.lock().await)();
        let transcriber = self.open_transcriber().await;
        let script = self.settings.lock().await.chinese_script;
        let (segments_tx, segments_rx) = mpsc::channel(256);

        match audio::recorder::Recorder::start(
            source,
            self.audio_dir(),
            mirror::id8(&meeting.id),
            transcriber,
            Some(segments_tx),
            script,
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
        let markdown = mirror::render(&meeting, &segments);
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
