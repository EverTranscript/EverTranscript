//! Recording a Meeting end to end, driven through the AudioSource seam.
//!
//! A scripted source stands in for hardware, so the whole vertical — Meeting
//! row, capture, joiner, ffmpeg sink, audio path in the record, Mirror
//! frontmatter — is exercised without a microphone or a TCC grant.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use evertranscript_core::Core;
use evertranscript_core::Server;
use evertranscript_core::audio::fixture::FixtureSource;
use evertranscript_core::audio::fixture::Step;
use evertranscript_core::client::CoreClient;
use evertranscript_core::transport;
use evertranscript_protocol::AudioChannel;
use evertranscript_protocol::MeetingResponse;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct TestCore {
    socket_path: common::Endpoint,
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
        let socket_path = common::endpoint(dir.path());
        let history_dir = dir.path().join("History");

        let core = Core::with_history_dir_acknowledged(history_dir.clone()).expect("core");
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


#[tokio::test]
async fn recording_a_meeting_writes_audio_and_records_where_it_went() {
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
    let core = Core::with_history_dir_acknowledged(history_dir).expect("core");

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
    let dir = tempfile::tempdir().expect("tempdir");
    let history_dir = dir.path().join("History");

    // A Core that recorded and was killed without ever stopping the Meeting.
    let started = {
        let core = Core::with_history_dir_acknowledged(history_dir.clone()).expect("core");
        core.set_source_factory(Arc::new(|| Box::new(FixtureSource::simple(2_000))))
            .await;
        let meeting = core
            .start_meeting(None, Some("Zoom".to_string()))
            .await
            .expect("start");
        // Wait for audio to actually reach disk. Since ADR-0032's reversal
        // there is no checkpoint to seal: the encoder writes continuously, so
        // the file exists and grows from the first block.
        for _ in 0..100 {
            if audio_bytes(&history_dir, &meeting.id) > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        meeting.id
        // Dropped without stop_meeting(): exactly what kill -9 leaves.
    };

    let bytes = audio_bytes(&history_dir, &started);
    assert!(
        bytes > 0,
        "a killed Core must leave a playable file, not a directory of fragments"
    );

    let core = Core::with_history_dir_acknowledged(history_dir.clone()).expect("core");
    core.reconcile_after_restart().await;

    let meetings = core.list_meetings(10, 0).await.expect("list");
    let meeting = meetings.first().expect("the interrupted Meeting survives");
    assert_eq!(meeting.id, started, "and it is the same one");

    // Leaving the bytes on disk was never the hard part; pointing the record
    // at them is what did not happen before.
    let audio = meeting
        .audio_path
        .as_ref()
        .expect("recovered audio must be attached to its Meeting");
    assert!(
        history_dir.join(audio).exists(),
        "and the path in the record must lead to a real file, got {audio}"
    );

    // The other half of a kill: the row was left open.
    assert!(
        meeting.ended_at.is_some(),
        "an interrupted Meeting must be closed rather than left running"
    );
    assert!(
        meeting
            .audio_notes
            .iter()
            .any(|note| note.contains("interrupted")),
        "and must say it was cut short, got {:?}",
        meeting.audio_notes
    );
}

/// Bytes of kept audio on disk for a Meeting, keyed the way the sink keys it.
fn audio_bytes(history_dir: &std::path::Path, meeting_id: &str) -> u64 {
    // The same marker the sink uses, rather than a copy of the rule: this
    // helper hard-coded 8 and silently stopped finding files when the marker
    // widened to 12 to stop back-to-back Meetings sharing a filename.
    let key = evertranscript_core::mirror::short_id(meeting_id);
    std::fs::metadata(history_dir.join(format!(".data/audio/{key}.mp3")))
        .map(|meta| meta.len())
        .unwrap_or(0)
}

#[tokio::test]
async fn a_meeting_that_lost_a_capture_leg_says_so_in_its_record_and_its_mirror() {
    // The failure this closes: capture loss reached a log line and stopped
    // there. A meeting recorded with no system audio produced a transcript
    // with one side of the conversation missing and nothing anywhere to
    // explain it — indistinguishable, to the person reading their notes
    // later, from a meeting where nobody else spoke. On a machine without
    // the system-audio permission that is every meeting.
    let core = TestCore::start(vec![
        Step::audio(AudioChannel::Mic, 200, 0.5),
        Step::Unavailable {
            channel: AudioChannel::System,
            reason: "permission to record system audio has not been granted".to_string(),
        },
        Step::audio(AudioChannel::Mic, 200, 0.5),
    ])
    .await;
    let mut client = core.client().await;

    let started: MeetingResponse = client.request("meeting/start", None).await.expect("start");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    client
        .request::<MeetingResponse>("meeting/stop", None)
        .await
        .expect("stop");

    let (meeting, _) = core
        .core
        .get_meeting(&started.meeting.id)
        .await
        .expect("get")
        .expect("the Meeting");
    assert!(
        meeting
            .audio_notes
            .iter()
            .any(|note| note.contains("system audio")),
        "the record must name the leg it lost, got {:?}",
        meeting.audio_notes
    );
    assert!(
        meeting
            .audio_notes
            .iter()
            .any(|note| note.contains("permission")),
        "and carry the reason the Operator can act on, got {:?}",
        meeting.audio_notes
    );

    core.core.mirror().rebuild_pending().await.expect("mirror");
    let body = core.mirror_body();
    assert!(
        body.contains("This recording is incomplete"),
        "the Mirror is what the Operator actually reads:\n{body}"
    );
}
