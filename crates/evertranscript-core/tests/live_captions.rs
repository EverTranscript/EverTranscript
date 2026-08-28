//! Live captions as a Client experiences them: subscribe, receive, and
//! attach mid-meeting without losing anything (ADR-0028).

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Arc;

use evertranscript_core::asr::FakeTranscriber;
use evertranscript_core::audio::fixture::FixtureSource;
use evertranscript_core::audio::fixture::Step;
use evertranscript_core::client::CoreClient;
use evertranscript_core::transport;
use evertranscript_core::Core;
use evertranscript_core::Server;
use evertranscript_protocol::AudioChannel;
use evertranscript_protocol::MeetingResponse;
use evertranscript_protocol::TranscriptSnapshotResponse;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct TestCore {
    socket_path: PathBuf,
    core: Arc<Core>,
    shutdown: CancellationToken,
    _dir: tempfile::TempDir,
}

impl TestCore {
    async fn start(lines: &'static [&'static str]) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("s");
        let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");

        // Enough speech, with pauses, to close several chunks.
        core.set_source_factory(Arc::new(|| {
            Box::new(FixtureSource::new(vec![
                Step::audio(AudioChannel::Mic, 4_000, 0.3),
                Step::audio(AudioChannel::Mic, 1_000, 0.0),
                Step::audio(AudioChannel::Mic, 4_000, 0.3),
                Step::audio(AudioChannel::Mic, 1_000, 0.0),
                Step::audio(AudioChannel::Mic, 4_000, 0.3),
                Step::audio(AudioChannel::Mic, 1_000, 0.0),
            ]))
        }))
        .await;
        core.set_transcriber_factory(Arc::new(move || {
            Some(Box::new(FakeTranscriber::with(lines.iter().copied())))
        }))
        .await;

        let listener = transport::bind(&socket_path).await.expect("bind");
        let (events_tx, events_rx) = mpsc::channel(64);
        let shutdown = CancellationToken::new();
        tokio::spawn(Server::new(Arc::clone(&core)).run(events_rx, shutdown.clone()));
        tokio::spawn(transport::serve(listener, events_tx, shutdown.clone()));

        Self {
            socket_path,
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
}

impl Drop for TestCore {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

/// Collects caption notifications until `count` arrive or time runs out.
async fn collect_captions(client: &mut CoreClient, count: usize) -> Vec<String> {
    let mut texts = Vec::new();
    let deadline = std::time::Duration::from_secs(3);
    let _ = tokio::time::timeout(deadline, async {
        while texts.len() < count {
            match client.next_notification().await {
                Ok(Some(notification)) if notification.method == "transcript/segmentAdded" => {
                    if let Some(params) = notification.params {
                        if let Some(text) = params
                            .get("segment")
                            .and_then(|segment| segment.get("text"))
                            .and_then(|text| text.as_str())
                        {
                            texts.push(text.to_string());
                        }
                    }
                }
                Ok(Some(_)) => continue,
                Ok(None) | Err(_) => break,
            }
        }
    })
    .await;
    texts
}

#[tokio::test]
async fn subscribing_delivers_live_captions() {
    let core = TestCore::start(&["first thing said", "second thing said", "third"]).await;
    let mut client = core.client().await;

    client
        .request::<MeetingResponse>("meeting/start", None)
        .await
        .expect("start");
    let snapshot: TranscriptSnapshotResponse = client
        .request("transcript/subscribe", None)
        .await
        .expect("subscribe");
    assert!(snapshot.subscribed);
    assert!(
        snapshot.meeting.is_some(),
        "subscribing during a recording should name the Meeting"
    );

    let captions = collect_captions(&mut client, 2).await;
    assert!(
        !captions.is_empty(),
        "captions must reach a subscribed Client"
    );
    assert!(
        captions.iter().any(|text| text.contains("said")),
        "captions should carry the transcribed text, got {captions:?}"
    );
}

#[tokio::test]
async fn a_client_that_never_subscribes_gets_no_captions() {
    // The CLI runs `search` on the same socket while a meeting records;
    // pushing every word at it would be noise.
    let core = TestCore::start(&["something", "else"]).await;
    let mut client = core.client().await;

    client
        .request::<MeetingResponse>("meeting/start", None)
        .await
        .expect("start");
    let captions = collect_captions(&mut client, 1).await;
    assert!(captions.is_empty(), "captions are opt-in, got {captions:?}");
}

#[tokio::test]
async fn attaching_mid_meeting_returns_the_transcript_so_far() {
    // Story 24: opening the app during a meeting must lose nothing. The
    // snapshot and the subscription are one call precisely so a segment
    // completing between them cannot slip through the gap.
    let core = TestCore::start(&["earlier words", "later words", "final words"]).await;
    let mut first = core.client().await;

    first
        .request::<MeetingResponse>("meeting/start", None)
        .await
        .expect("start");
    // Let some transcript accumulate before anyone is watching.
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    let mut late = core.client().await;
    let snapshot: TranscriptSnapshotResponse = late
        .request("transcript/subscribe", None)
        .await
        .expect("subscribe");

    assert!(
        !snapshot.segments.is_empty(),
        "a Client attaching mid-meeting must receive what it missed"
    );
    assert!(snapshot.subscribed, "and be subscribed from that instant");
    let meeting = snapshot.meeting.expect("the running Meeting");
    assert!(meeting.ended_at.is_none(), "it is still recording");
}

#[tokio::test]
async fn unsubscribing_stops_delivery() {
    let core = TestCore::start(&["one", "two", "three", "four"]).await;
    let mut client = core.client().await;

    client
        .request::<MeetingResponse>("meeting/start", None)
        .await
        .expect("start");
    client
        .request::<TranscriptSnapshotResponse>("transcript/subscribe", None)
        .await
        .expect("subscribe");
    let response: evertranscript_protocol::TranscriptUnsubscribeResponse = client
        .request("transcript/unsubscribe", None)
        .await
        .expect("unsubscribe");
    assert!(!response.subscribed);

    let captions = collect_captions(&mut client, 1).await;
    assert!(
        captions.is_empty(),
        "no captions after unsubscribing, got {captions:?}"
    );
}

#[tokio::test]
async fn transcript_segments_are_persisted_not_just_broadcast() {
    // A caption the Client saw but the record never stored would be a
    // Meeting that looks transcribed live and empty afterwards.
    let core = TestCore::start(&["persisted words", "more persisted words"]).await;
    let mut client = core.client().await;

    let started: MeetingResponse = client.request("meeting/start", None).await.expect("start");
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    client
        .request::<MeetingResponse>("meeting/stop", None)
        .await
        .expect("stop");

    let (_, segments) = core
        .core
        .get_meeting(&started.meeting.id)
        .await
        .expect("get")
        .expect("the Meeting");
    assert!(
        !segments.is_empty(),
        "what was captioned must also be in the record"
    );
    assert!(
        segments
            .iter()
            .all(|segment| segment.channel == AudioChannel::Mic),
        "only the mic leg had audio"
    );
}
