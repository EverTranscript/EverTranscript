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
    fail_with: Option<String>,
}

impl FixtureDiarizer {
    pub fn new(turns: Vec<Turn>) -> Self {
        let embeddings = synthetic_embeddings(&turns);
        Self {
            diarization: Diarization { turns, embeddings },
            steps: 4,
            fail_with: None,
        }
    }

    /// A Diarizer whose models are missing. The Transcript must survive this
    /// unattributed rather than the Meeting failing (ticket 03).
    pub fn unavailable(reason: &str) -> Self {
        Self {
            diarization: Diarization::default(),
            steps: 0,
            fail_with: Some(reason.to_string()),
        }
    }

    /// Drop a cluster's embedding, as the real pipeline does when a voice
    /// has too little clean audio to embed honestly.
    pub fn without_embedding_for(mut self, cluster: u32) -> Self {
        self.diarization.embeddings.remove(&super::Cluster(cluster));
        self
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

    /// Two voices talking over each other.
    ///
    /// Overlap is where turn boundaries are least trustworthy, which makes
    /// it where reconciliation's boundary-flip metric earns its keep.
    pub fn overlapped() -> Self {
        Self::new(vec![
            Turn::new(AudioChannel::System, 0, 6_000, 0),
            Turn::new(AudioChannel::System, 4_000, 9_000, 1),
            Turn::new(AudioChannel::Mic, 5_000, 7_500, 2),
        ])
    }

    /// A turn shorter than any sane embedding window.
    ///
    /// "Mm-hm" is not noise to be dropped — it is a real turn by a real
    /// person, and the honest outcome is a cluster with no embedding rather
    /// than either a fabricated Voiceprint or a silently discarded turn.
    pub fn very_short_turn() -> Self {
        Self::new(vec![
            Turn::new(AudioChannel::System, 0, 8_000, 0),
            Turn::new(AudioChannel::Mic, 8_100, 8_400, 1),
            Turn::new(AudioChannel::System, 8_600, 15_000, 0),
        ])
        .without_embedding_for(1)
    }
}

/// Deterministic, well-separated vectors — one direction per cluster.
///
/// Orthogonal on purpose: the fixture's job is to let policy tests assert
/// "these two are the same voice" without also testing whether a cosine
/// threshold is well chosen. Anything that needs to reason about vectors that are genuinely close
/// belongs in the real pipeline's tests.
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
            (
                super::Cluster(cluster),
                Embedding::new(vector, FIXTURE_MODEL, FIXTURE_MODEL_VERSION),
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
        if let Some(reason) = &self.fail_with {
            return Err(DiarizeError::Unavailable(reason.clone()));
        }

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

#[cfg(test)]
mod tests {
    use super::*;

    fn run(diarizer: &mut dyn Diarizer) -> Result<Diarization, DiarizeError> {
        let silence = vec![0.0_f32; 16_000];
        let audio = MeetingAudio {
            mic: &silence,
            system: &silence,
            sample_rate: 16_000,
        };
        diarizer.diarize(audio, &mut |_| {}, &Cancel::new())
    }

    #[test]
    fn the_shared_room_puts_two_voices_on_the_mic_channel() {
        // The case ADR-0029's amendment exists for. If this fixture ever
        // collapses to one mic cluster, ticket 05 loses the only test that
        // can falsify "the mic channel is the Operator" as an axiom.
        let result = run(&mut FixtureDiarizer::shared_room()).expect("runs");
        let mut mic_clusters: Vec<_> = result
            .turns
            .iter()
            .filter(|turn| turn.channel == AudioChannel::Mic)
            .map(|turn| turn.cluster)
            .collect();
        mic_clusters.sort_unstable();
        mic_clusters.dedup();
        assert_eq!(mic_clusters.len(), 2, "a room has more than the Operator");
    }

    #[test]
    fn the_solo_meeting_is_one_voice_for_a_long_time() {
        // Long enough that a clusterer which invents speakers from drift has
        // room to do it.
        let result = run(&mut FixtureDiarizer::solo()).expect("runs");
        assert_eq!(result.clusters().len(), 1);
        assert!(
            result.turns.last().expect("turns").end.millis() > 45 * 60 * 1_000,
            "the point of this fixture is its length"
        );
    }

    #[test]
    fn a_turn_too_short_to_embed_still_exists_as_a_turn() {
        // The honest outcome for "mm-hm": a real turn, no Voiceprint.
        // Dropping the turn would lose a real person's real words, and
        // fabricating an embedding would poison a Speaker.
        let result = run(&mut FixtureDiarizer::very_short_turn()).expect("runs");
        let short = result
            .turns
            .iter()
            .find(|turn| turn.duration_ms() < 500)
            .expect("the short turn survives");
        assert!(
            !result.embeddings.contains_key(&short.cluster),
            "no Voiceprint is claimed from 300ms of audio"
        );
    }

    #[test]
    fn overlapped_speech_produces_overlapping_turns() {
        // Two turns covering the same instant on the same channel. Any code
        // that assumed turns partition the timeline is wrong here, which is
        // exactly why the fixture ships this shape.
        let result = run(&mut FixtureDiarizer::overlapped()).expect("runs");
        let at = crate::audio::CaptureOffset(5_000);
        let covering = result
            .turns
            .iter()
            .filter(|turn| turn.channel == AudioChannel::System && turn.contains(at))
            .count();
        assert_eq!(covering, 2, "both voices are speaking");
    }

    #[test]
    fn cancellation_stops_a_run_rather_than_returning_a_partial_answer() {
        // Cancelled is not a degraded success. A caller that got half a
        // Diarization back would write half an attribution over a Meeting.
        let cancel = Cancel::new();
        cancel.cancel();
        let silence = vec![0.0_f32; 16_000];
        let audio = MeetingAudio {
            mic: &silence,
            system: &silence,
            sample_rate: 16_000,
        };
        let result = FixtureDiarizer::clean_two_speaker().diarize(audio, &mut |_| {}, &cancel);
        assert!(matches!(result, Err(DiarizeError::Cancelled)));
    }

    #[test]
    fn progress_reaches_the_end_of_the_meeting() {
        let mut seen = Vec::new();
        let silence = vec![0.0_f32; 16_000];
        let audio = MeetingAudio {
            mic: &silence,
            system: &silence,
            sample_rate: 16_000,
        };
        FixtureDiarizer::clean_two_speaker()
            .diarize(audio, &mut |progress| seen.push(progress), &Cancel::new())
            .expect("runs");
        assert_eq!(seen.last().map(|p| p.fraction()), Some(1.0));
    }

    #[test]
    fn a_missing_model_is_unavailable_rather_than_a_failure_to_diarize() {
        // Ticket 03: the Transcript stays unattributed and says so. The
        // caller must be able to tell this apart from "ran and found
        // nobody", which is a legitimate answer for a silent recording.
        let result = run(&mut FixtureDiarizer::unavailable("model not downloaded"));
        assert!(matches!(result, Err(DiarizeError::Unavailable(_))));
    }

    #[test]
    fn fixture_embeddings_name_their_space() {
        // Comparing a fixture vector to a real one must fail on the model
        // mismatch rather than quietly returning a number.
        let result = run(&mut FixtureDiarizer::clean_two_speaker()).expect("runs");
        let embedding = result.embeddings.values().next().expect("some embedding");
        assert_eq!(embedding.model, FIXTURE_MODEL);
    }
}
