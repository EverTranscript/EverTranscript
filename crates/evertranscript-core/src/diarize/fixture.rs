//! A Diarizer that replays a scripted timeline instead of running models.
//!
//! Every clustering, persistence and naming test in M3 drives this rather
//! than ONNX, for the same reason M1's policy tests drove `FixtureSource` and
//! M2's drove `FixtureDetectionSource`: the decisions worth testing are about
//! Speakers and History, and pinning them to two model files would make them
//! slow, flaky, and untestable on a machine without the downloads.
//!
//! **The timelines here are deliberately unpleasant.** M1's chunker passed
//! every whole-file fixture and discarded every sample from a real
//! microphone; M2's detection policy was only correct under dribbled events
//! because the fixture could dribble. The diarization form of that bug is a
//! clusterer that is right about a tidy three-way conversation and wrong
//! about fifty minutes of one person, or about the half-second "mm-hm" that
//! is shorter than an embedding window. Those cases are constructors below,
//! not something a later session has to think to write.

use std::collections::BTreeMap;

use evertranscript_protocol::AudioChannel;

use super::Cancel;
use super::Diarization;
use super::DiarizeError;
use super::Diarizer;
use super::Embedding;
use super::MeetingAudio;
use super::Progress;
use super::Turn;

/// The model name fixture embeddings claim.
///
/// Named rather than empty so a test that accidentally compares a fixture
/// vector against a real one fails on the model mismatch — loudly — instead
/// of computing a cosine between two unrelated spaces.
pub const FIXTURE_MODEL: &str = "fixture";
pub const FIXTURE_MODEL_VERSION: &str = "1";

/// A Diarizer that returns what it was told to return.
pub struct FixtureDiarizer {
    diarization: Diarization,
    /// Progress ticks to emit before finishing, so a test can observe a
    /// cancellation partway through a long job rather than only before it.
    steps: usize,
}

impl FixtureDiarizer {
    pub fn new(turns: Vec<Turn>) -> Self {
        let embeddings = synthetic_embeddings(&turns);
        Self {
            diarization: Diarization { turns, embeddings },
            steps: 4,
        }
    }

    pub fn with_steps(mut self, steps: usize) -> Self {
        self.steps = steps;
        self
    }

    // ---- The timelines ticket 01 requires ----

    /// Two people alternating cleanly on the system channel, with the
    /// Operator on the mic. The easy case, and the only one most
    /// implementations get right.
    pub fn clean_two_speaker() -> Self {
        Self::new(vec![
            Turn::new(AudioChannel::Mic, 0, 3_000, 0),
            Turn::new(AudioChannel::System, 3_200, 8_000, 1),
            Turn::new(AudioChannel::Mic, 8_200, 11_000, 0),
            Turn::new(AudioChannel::System, 11_200, 16_000, 2),
            Turn::new(AudioChannel::System, 16_500, 21_000, 1),
        ])
    }

    /// One person, fifty minutes, nobody else. A clusterer tuned on
    /// conversations tends to invent a second speaker here out of nothing
    /// but microphone drift.
    pub fn solo() -> Self {
        let mut turns = Vec::new();
        let mut at = 0;
        while at < 50 * 60 * 1_000 {
            turns.push(Turn::new(AudioChannel::Mic, at, at + 20_000, 0));
            at += 22_000;
        }
        Self::new(turns)
    }

    /// A shared conference room: two real voices on the **mic** channel.
    ///
    /// This is the case ADR-0029's amendment exists for. A design that
    /// treats the mic channel as the Operator by axiom rather than by prior
    /// silently mis-attributes every word the colleague across the table
    /// says, and no test that only ever puts one voice on the mic will
    /// notice.
    pub fn shared_room() -> Self {
        Self::new(vec![
            Turn::new(AudioChannel::Mic, 0, 4_000, 0),
            Turn::new(AudioChannel::Mic, 4_500, 9_000, 1),
            Turn::new(AudioChannel::System, 9_500, 14_000, 2),
            Turn::new(AudioChannel::Mic, 14_500, 17_000, 0),
            Turn::new(AudioChannel::Mic, 17_200, 20_000, 1),
        ])
    }

    /// A voice that already has a Voiceprint from an earlier Meeting.
    ///
    /// The fixture cannot know that by itself — recognition is policy above
    /// the seam — so this is just a timeline whose cluster 1 the *test*
    /// seeds with a prior Speaker. Story 28 is the assertion that it comes
    /// back as the same Speaker rather than a new one.
    pub fn returning_speaker() -> Self {
        Self::new(vec![
            Turn::new(AudioChannel::Mic, 0, 5_000, 0),
            Turn::new(AudioChannel::System, 5_500, 12_000, 1),
            Turn::new(AudioChannel::Mic, 12_500, 15_000, 0),
        ])
    }
}

/// Deterministic, well-separated vectors — one direction per cluster.
///
/// Orthogonal on purpose: the fixture's job is to let policy tests assert
/// "these two are the same voice" without also testing whether a cosine
/// threshold is well chosen. Anything that needs to reason about vectors that are genuinely close
/// belongs in the real pipeline's tests.
///
/// Each vector reports the voice's real length — the sum of its turns — so
/// a policy that keys on how much of a voice there was sees the timeline's
/// shape rather than a placeholder.
fn synthetic_embeddings(turns: &[Turn]) -> BTreeMap<super::Cluster, Embedding> {
    let mut clusters: Vec<u32> = turns.iter().map(|turn| turn.cluster.index()).collect();
    clusters.sort_unstable();
    clusters.dedup();

    let width = clusters.iter().copied().max().unwrap_or(0) as usize + 1;
    clusters
        .into_iter()
        .map(|cluster| {
            let mut vector = vec![0.0_f32; width.max(2)];
            vector[cluster as usize] = 1.0;
            let voiced_ms = turns
                .iter()
                .filter(|turn| turn.cluster.index() == cluster)
                .map(|turn| turn.duration_ms())
                .sum();
            (
                super::Cluster(cluster),
                Embedding::new(vector, FIXTURE_MODEL, FIXTURE_MODEL_VERSION, voiced_ms),
            )
        })
        .collect()
}

impl Diarizer for FixtureDiarizer {
    fn diarize(
        &mut self,
        audio: MeetingAudio<'_>,
        progress: &mut dyn FnMut(Progress),
        cancel: &Cancel,
    ) -> Result<Diarization, DiarizeError> {
        // Report the same shape a real run does, and honour cancellation at
        // the same granularity, so a test can prove the caller handles a job
        // that stops halfway.
        let total_ms = self
            .diarization
            .turns
            .iter()
            .map(|turn| turn.end.millis())
            .max()
            .unwrap_or_else(|| audio.duration_ms());

        for step in 0..=self.steps {
            if cancel.is_cancelled() {
                return Err(DiarizeError::Cancelled);
            }
            let done_ms = if self.steps == 0 {
                total_ms
            } else {
                total_ms * step as u64 / self.steps as u64
            };
            progress(Progress { done_ms, total_ms });
        }

        Ok(self.diarization.clone())
    }

    fn describe(&self) -> String {
        format!(
            "fixture diarizer ({} turns, {} clusters)",
            self.diarization.turns.len(),
            self.diarization.clusters().len()
        )
    }
}
