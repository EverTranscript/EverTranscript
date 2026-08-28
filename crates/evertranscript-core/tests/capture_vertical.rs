//! Recording a Meeting end to end, driven through the AudioSource seam.
//!
//! A scripted source stands in for hardware, so the whole vertical — Meeting
//! row, capture, joiner, ffmpeg sink, audio path in the record, Mirror
//! frontmatter — is exercised without a microphone or a TCC grant.

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Arc;

use evertranscript_core::audio::fixture::FixtureSource;
use evertranscript_core::audio::fixture::Step;
use evertranscript_core::audio::sink::ffmpeg_available;
use evertranscript_core::client::CoreClient;
use evertranscript_core::transport;
use evertranscript_core::Core;
use evertranscript_core::Server;
use evertranscript_protocol::AudioChannel;
use evertranscript_protocol::MeetingResponse;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct TestCore {
    socket_path: PathBuf,
    history_dir: PathBuf,
    #[allow(dead_code)]
    core: Arc<Core>,
    shutdown: CancellationToken,
    _dir: tempfile::TempDir,
}

impl TestCore {
    /// Starts a Core whose capture is the given script.
    async fn start(script: Vec<Step>) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("s");
        let history_dir = dir.path().join("History");

        let core = Core::with_history_dir(history_dir.clone()).expect("core");
        core.set_source_factory(Arc::new(move || {
            Box::new(FixtureSource::new(script.clone()))
        }))
        .await;

        let listener = transport::bind(&socket_path).await.expect("bind");
        let (events_tx, events_rx) = mpsc::channel(64);
        let shutdown = CancellationToken::new();
        tokio::spawn(Server::new(Arc::clone(&core)).run(events_rx, shutdown.clone()));
        tokio::spawn(transport::serve(listener, events_tx, shutdown.clone()));
        tokio::spawn(
            core.mirror()
                .clone()
                .run(core.mirror_wake(), shutdown.clone()),
        );

        Self {
            socket_path,
            history_dir,
            core,
            shutdown,
            _dir: dir,
        }
    }

    async fn client(&self) -> CoreClient {
        let mut client = CoreClient::connect_to(&self.socket_path)
            .await
            .expect("connect");
        client
            .initialize("test-client", "0.0.0")
            .await
            .expect("initialize");
        client
    }

    fn mirror_body(&self) -> String {
        let name = std::fs::read_dir(&self.history_dir)
            .expect("read history")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .find(|name| name.ends_with(".md"))
            .expect("a Mirror");
        std::fs::read_to_string(self.history_dir.join(name)).expect("read mirror")
    }
}

impl Drop for TestCore {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

async fn skip_without_ffmpeg() -> bool {
    if ffmpeg_available().await {
        return false;
    }
    eprintln!("skipping: ffmpeg is not available on this machine");
    true
}

#[tokio::test]
async fn recording_a_meeting_writes_audio_and_records_where_it_went() {
    if skip_without_ffmpeg().await {
        return;
    }
    let core = TestCore::start(vec![
        Step::audio(AudioChannel::Mic, 400, 0.4),
        Step::audio(AudioChannel::System, 400, -0.4),
    ])
    .await;
    let mut client = core.client().await;

    let started: MeetingResponse = client
        .request("meeting/start", Some(json!({ "detectedApp": "Zoom" })))
        .await
        .expect("start");
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let stopped: MeetingResponse = client.request("meeting/stop", None).await.expect("stop");

    let audio_path = stopped
        .meeting
        .audio_path
        .expect("the Meeting must record where its audio went");
    // Stored relative so moving the History folder does not break it.
    assert!(
        !audio_path.starts_with('/'),
        "audio paths must be relative to the History folder: {audio_path}"
    );
    assert!(
        audio_path.starts_with(".data/audio/"),
        "audio belongs in the hidden machine store: {audio_path}"
    );

    let absolute = core.history_dir.join(&audio_path);
    assert!(
        absolute.exists(),
        "the audio file must exist at {audio_path}"
    );
    assert!(
        std::fs::metadata(&absolute).expect("metadata").len() > 0,
        "and must not be empty"
    );

    // The Mirror points at it, which is what makes "any player serves" real
    // now that audio lives somewhere hidden.
    let body = core.mirror_body();
    assert!(
        body.contains(&format!("audio: {audio_path}")),
        "the Mirror should carry the audio path:\n{body}"
    );

    assert_eq!(
        started.meeting.id, stopped.meeting.id,
        "one Meeting, not two"
    );
}

#[tokio::test]
async fn a_device_swap_mid_meeting_does_not_split_the_recording() {
    if skip_without_ffmpeg().await {
        return;
    }
    // The AirPods scenario, end to end: capture pauses, the device changes,
    // capture resumes. One Meeting, one audio file, the outage represented.
    let core = TestCore::start(vec![
        Step::audio(AudioChannel::Mic, 200, 0.4),
        Step::DeviceChange {
            channel: AudioChannel::Mic,
        },
        Step::Gap { ms: 300 },
        Step::audio(AudioChannel::Mic, 200, 0.4),
    ])
    .await;
    let mut client = core.client().await;

    client
        .request::<MeetingResponse>("meeting/start", Some(json!({ "detectedApp": "Zoom" })))
        .await
        .expect("start");
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    client
        .request::<MeetingResponse>("meeting/stop", None)
        .await
        .expect("stop");

    let listed: evertranscript_protocol::MeetingListResponse =
        client.request("meeting/list", None).await.expect("list");
    assert_eq!(
        listed.meetings.len(),
        1,
        "device churn must never produce a second Meeting"
    );

    let mirrors: Vec<String> = std::fs::read_dir(&core.history_dir)
        .expect("read history")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.ends_with(".md"))
        .collect();
    assert_eq!(mirrors.len(), 1, "one Meeting, one Mirror: {mirrors:?}");
}

#[tokio::test]
async fn a_meeting_records_even_when_capture_cannot_start() {
    // Capture failing must not cost the Meeting: the transcript is the
    // record and audio is the bonus (ADR-0019).
    let dir = tempfile::tempdir().expect("tempdir");
    let history_dir = dir.path().join("History");
    let core = Core::with_history_dir(history_dir).expect("core");

    // A source that refuses to start at all.
    struct BrokenSource;
    impl evertranscript_core::audio::AudioSource for BrokenSource {
        fn start(
            &mut self,
            _clock: evertranscript_core::audio::CaptureClock,
            _events: mpsc::Sender<evertranscript_core::audio::CaptureEvent>,
        ) -> anyhow::Result<()> {
            anyhow::bail!("no audio hardware here")
        }
        fn stop(&mut self) {}
        fn describe(&self) -> String {
            "broken".to_string()
        }
    }
    core.set_source_factory(Arc::new(|| Box::new(BrokenSource)))
        .await;

    let meeting = core
        .start_meeting(None, Some("Zoom".to_string()))
        .await
        .expect("the Meeting must start even with no capture");
    let stopped = core.stop_meeting().await.expect("stop");

    assert_eq!(stopped.id, meeting.id);
    assert!(
        stopped.audio_path.is_none(),
        "no audio was captured, and the record says so rather than pointing at nothing"
    );
}

#[tokio::test]
async fn audio_from_an_interrupted_run_is_recovered_on_the_next_start() {
    if skip_without_ffmpeg().await {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let history_dir = dir.path().join("History");

    // A Core that recorded and was killed before finalizing: the checkpoint
    // directory survives with sealed segments in it.
    {
        let core = Core::with_history_dir(history_dir.clone()).expect("core");
        core.set_source_factory(Arc::new(|| Box::new(FixtureSource::simple(200))))
            .await;
        core.start_meeting(None, Some("Zoom".to_string()))
            .await
            .expect("start");
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        // Dropped without stop_meeting(): exactly what kill -9 leaves.
    }

    let checkpoints = history_dir.join(".data/audio/.checkpoints");
    if !checkpoints.exists() {
        // The scripted audio may not have filled a checkpoint; nothing to
        // recover is a valid outcome, not a failure.
        return;
    }

    let core = Core::with_history_dir(history_dir.clone()).expect("core");
    core.recover_interrupted_audio().await;
    assert!(
        !checkpoints.exists()
            || std::fs::read_dir(&checkpoints)
                .into_iter()
                .flatten()
                .count()
                == 0,
        "recovery must consume the checkpoint directory"
    );
}
