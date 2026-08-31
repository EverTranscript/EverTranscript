//! Diarization: who spoke, and nothing about what was said.
//!
//! This is the M3 twin of [`crate::audio`]'s AudioSource and
//! [`crate::detect`]'s DetectionSource. Both proved the same shape — one
//! trait between the product and the expensive, unreliable thing, and
//! everything above it becomes testable without it. Here the expensive thing
//! is a pair of ONNX models and a whole meeting's audio, so the alternative
//! to a seam is a clustering policy that can only be exercised by recording a
//! conversation.
//!
//! Three things are load-bearing:
//!
//! 1. **The seam produces clusters, not Speakers.** A [`Cluster`] is "this
//!    voice, within this Meeting". Turning it into a persistent Speaker is
//!    policy above the seam, because that decision needs History — every
//!    other Meeting's Voiceprints — and a model that needed History would be
//!    a model that cannot be tested on one file.
//! 2. **Turns carry the capture clock.** [`CaptureOffset`] is the same clock
//!    ASR words already carry (ADR-0029), which is what makes reconciliation
//!    an interval-overlap problem with one right answer instead of a
//!    correlation problem with a plausible one. A second clock here would
//!    make attribution unprovable rather than merely wrong.
//! 3. **Diarization is post-meeting, long, and interruptible.** It runs over
//!    a finished recording while the Operator is doing something else, so it
//!    reports progress and takes cancellation — an unaccountable multi-minute
//!    job on someone's laptop is not a thing this product gets to have.
//!
//! What this milestone must not repeat: M1's chunker was correct against
//! whole-file fixtures and discarded every sample a live microphone gave it,
//! because fixtures arrive whole and hardware arrives in fragments. The
//! diarization form of that is a clusterer that is correct on tidy
//! three-way conversations and wrong on the meeting that is one person for
//! fifty minutes. [`fixture`] exists to produce the ugly shapes on purpose.

pub mod fixture;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use evertranscript_protocol::AudioChannel;

use crate::audio::CaptureOffset;

/// One voice within one Meeting, before it is anyone in particular.
///
/// Deliberately not a Speaker id. The seam's job ends at "these turns are
/// the same voice"; deciding *whose* voice needs every other Meeting's
/// Voiceprints, and that lookup belongs above here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cluster(pub u32);

impl Cluster {
    pub fn index(self) -> u32 {
        self.0
    }
}

/// One stretch of one voice on one channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Turn {
    pub channel: AudioChannel,
    pub start: CaptureOffset,
    pub end: CaptureOffset,
    pub cluster: Cluster,
}

impl Turn {
    pub fn new(channel: AudioChannel, start_ms: u64, end_ms: u64, cluster: u32) -> Self {
        Self {
            channel,
            start: CaptureOffset(start_ms),
            end: CaptureOffset(end_ms),
            cluster: Cluster(cluster),
        }
    }

    pub fn duration_ms(&self) -> u64 {
        self.end.millis().saturating_sub(self.start.millis())
    }

    /// Whether this turn contains an instant. Half-open on purpose: a word
    /// whose midpoint lands exactly on a boundary belongs to the turn that
    /// is starting, and exactly one turn can claim it.
    pub fn contains(&self, at: CaptureOffset) -> bool {
        at >= self.start && at < self.end
    }
}

/// A voice embedding, in whatever space the model that produced it uses.
///
/// Carries the model and version it came from because comparing vectors
/// across two embedding spaces produces a plausible number and a meaningless
/// one — the failure that makes recognition mysteriously degrade after a
/// model upgrade. ADR-0035 already puts these columns in the record for the
/// same reason.
#[derive(Debug, Clone, PartialEq)]
pub struct Embedding {
    pub vector: Vec<f32>,
    pub model: String,
    pub model_version: String,
}

impl Embedding {
    pub fn new(vector: Vec<f32>, model: &str, model_version: &str) -> Self {
        Self {
            vector,
            model: model.to_string(),
            model_version: model_version.to_string(),
        }
    }
}

/// What a [`Diarizer`] concluded about one Meeting.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Diarization {
    /// Ordered by start time, then channel. Callers depend on the ordering
    /// rather than re-sorting, so a source that emits out of order is a bug
    /// in the source.
    pub turns: Vec<Turn>,
    /// One durable identity embedding per cluster. A cluster with too little
    /// clean voiced audio to embed honestly is absent rather than present
    /// with a bad vector — a Voiceprint built from crosstalk is worse than
    /// no Voiceprint.
    pub embeddings: BTreeMap<Cluster, Embedding>,
}

impl Diarization {
    /// Every distinct cluster, in order.
    pub fn clusters(&self) -> Vec<Cluster> {
        let mut seen: Vec<Cluster> = self.turns.iter().map(|turn| turn.cluster).collect();
        seen.sort_unstable();
        seen.dedup();
        seen
    }

    /// The turn covering an instant on a channel, if any.
    ///
    /// The whole of reconciliation (ticket 06) is this function plus a
    /// midpoint, which is why it lives here rather than being open-coded
    /// wherever attribution happens.
    pub fn turn_at(&self, channel: AudioChannel, at: CaptureOffset) -> Option<&Turn> {
        self.turns
            .iter()
            .find(|turn| turn.channel == channel && turn.contains(at))
    }
}

/// The audio a Diarizer is given: one finished Meeting, both channels.
///
/// Borrowed rather than owned so the caller decides where a meeting's worth
/// of samples lives — an hour of one channel is a few hundred megabytes, and
/// a seam that forced a `Vec` would make streaming it impossible later
/// without changing every implementation.
#[derive(Debug, Clone, Copy)]
pub struct MeetingAudio<'a> {
    pub mic: &'a [f32],
    pub system: &'a [f32],
    pub sample_rate: u32,
}

impl MeetingAudio<'_> {
    /// The longer of the two channels, which is the Meeting's length.
    pub fn duration_ms(&self) -> u64 {
        let samples = self.mic.len().max(self.system.len()) as u64;
        if self.sample_rate == 0 {
            return 0;
        }
        samples * 1000 / self.sample_rate as u64
    }
}

/// How far along a diarization job is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Progress {
    pub done_ms: u64,
    pub total_ms: u64,
}

impl Progress {
    pub fn fraction(&self) -> f32 {
        if self.total_ms == 0 {
            return 1.0;
        }
        (self.done_ms as f32 / self.total_ms as f32).clamp(0.0, 1.0)
    }
}

/// A cancellation flag a running job checks.
///
/// Shared rather than passed by value because the thing that cancels is a
/// Client disconnecting or an Operator pressing stop, and it is never on the
/// thread doing the work.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Why a diarization run produced nothing.
#[derive(Debug, thiserror::Error)]
pub enum DiarizeError {
    /// The Operator or a disconnecting Client stopped it. Not a failure:
    /// the Meeting keeps whatever attribution had already been written, and
    /// nothing is half-applied.
    #[error("diarization cancelled")]
    Cancelled,
    /// A model is missing, corrupt, or refused to load. The Transcript stays
    /// unattributed and says so; it never costs the recording.
    #[error("diarization unavailable: {0}")]
    Unavailable(String),
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

/// Turns a finished Meeting's audio into speaker turns.
pub trait Diarizer: Send {
    /// Runs to completion, cancellation, or failure.
    ///
    /// Synchronous by design: unlike capture and detection, which produce
    /// events forever, this has an answer and then it is done. A test asserts
    /// on the returned value and never waits for one.
    fn diarize(
        &mut self,
        audio: MeetingAudio<'_>,
        progress: &mut dyn FnMut(Progress),
        cancel: &Cancel,
    ) -> Result<Diarization, DiarizeError>;

    /// For logs and errors.
    fn describe(&self) -> String;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_claims_an_instant_exactly_once() {
        // Half-open, so a word whose midpoint lands on a boundary belongs to
        // exactly one turn. Closed intervals would let two turns both claim
        // it and make attribution depend on iteration order.
        let first = Turn::new(AudioChannel::Mic, 0, 1_000, 0);
        let second = Turn::new(AudioChannel::Mic, 1_000, 2_000, 1);

        let boundary = CaptureOffset(1_000);
        assert!(!first.contains(boundary), "the ending turn releases it");
        assert!(second.contains(boundary), "the starting turn claims it");
    }

    #[test]
    fn a_turn_never_reports_a_negative_duration() {
        // A source that emits end before start must not produce a duration
        // near u64::MAX — the diarization form of the bug DetectionInstant
        // guards against.
        let backwards = Turn::new(AudioChannel::Mic, 500, 100, 0);
        assert_eq!(backwards.duration_ms(), 0);
    }

    #[test]
    fn attribution_is_per_channel() {
        // The same instant is two different voices on two channels, which is
        // the whole point of diarizing both (ADR-0029 as amended). A lookup
        // that ignored the channel would attribute the far end's words to
        // whoever was in the room.
        let diarization = Diarization {
            turns: vec![
                Turn::new(AudioChannel::Mic, 0, 5_000, 0),
                Turn::new(AudioChannel::System, 0, 5_000, 1),
            ],
            embeddings: BTreeMap::new(),
        };

        let at = CaptureOffset(2_500);
        assert_eq!(
            diarization
                .turn_at(AudioChannel::Mic, at)
                .map(|t| t.cluster),
            Some(Cluster(0))
        );
        assert_eq!(
            diarization
                .turn_at(AudioChannel::System, at)
                .map(|t| t.cluster),
            Some(Cluster(1))
        );
    }

    #[test]
    fn silence_between_turns_belongs_to_nobody() {
        // Diarization must be allowed to say "no one was speaking". A seam
        // that always returned some cluster would force reconciliation to
        // attribute silence to whoever spoke last.
        let diarization = Diarization {
            turns: vec![
                Turn::new(AudioChannel::Mic, 0, 1_000, 0),
                Turn::new(AudioChannel::Mic, 4_000, 5_000, 0),
            ],
            embeddings: BTreeMap::new(),
        };
        assert!(
            diarization
                .turn_at(AudioChannel::Mic, CaptureOffset(2_500))
                .is_none()
        );
    }

    #[test]
    fn a_cancelled_flag_is_visible_to_every_holder() {
        let cancel = Cancel::new();
        let watcher = cancel.clone();
        assert!(!watcher.is_cancelled());
        cancel.cancel();
        assert!(watcher.is_cancelled(), "the running job must see it");
    }

    #[test]
    fn progress_on_an_empty_meeting_is_complete_rather_than_undefined() {
        // A zero-length Meeting is a real case — a trigger that stopped
        // immediately — and dividing by its length is how a progress bar
        // becomes NaN.
        let progress = Progress {
            done_ms: 0,
            total_ms: 0,
        };
        assert_eq!(progress.fraction(), 1.0);
    }
}
