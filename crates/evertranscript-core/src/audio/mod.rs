//! Capture: two channels, one clock, crash-safe on disk.
//!
//! The architecture here is the ADR-0029 amendment made concrete. Three
//! things are load-bearing and none of them are obvious:
//!
//! 1. **Absolute capture timestamps.** Every frame is stamped when captured,
//!    on one clock shared by both channels. Audio position and transcript
//!    timing both derive from it, so a capture gap is explicit rather than
//!    silently shortening the file. Both open-source competitors omit this
//!    and ship an audio-vs-transcript drift that grows with every dropout.
//! 2. **The session owns the sink; capture streams are replaceable leaves.**
//!    A device swap replaces a stream, never the Meeting.
//! 3. **The legs are independent.** The system-audio leg failing must not
//!    stop the microphone, and neither may stop the recording.

pub mod aec;
pub mod dsp;
pub mod fixture;
pub mod joiner;
pub mod leg;
pub mod live;
pub mod recorder;
pub mod sink;
pub mod supervisor;
pub mod system;

use evertranscript_protocol::AudioChannel;

/// Capture sample rate. 48 kHz is what both platforms hand us natively;
/// the 16 kHz ASR needs is resampled downstream (ticket 06).
pub const SAMPLE_RATE: u32 = 48_000;

/// How much audio one frame carries. Small enough for responsive captions,
/// large enough that per-frame overhead is irrelevant.
pub const FRAME_MS: u64 = 20;

pub const SAMPLES_PER_FRAME: usize = (SAMPLE_RATE as u64 * FRAME_MS / 1000) as usize;

/// Milliseconds since the Meeting's capture clock started.
///
/// A newtype rather than a bare u64 because mixing this with a sample index
/// or a wall-clock epoch is exactly the confusion that produces drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CaptureOffset(pub u64);

impl CaptureOffset {
    pub const ZERO: Self = Self(0);

    pub fn millis(&self) -> u64 {
        self.0
    }

    /// Sample index this offset lands on, at the capture rate.
    pub fn sample_index(&self) -> u64 {
        self.0 * SAMPLE_RATE as u64 / 1000
    }

    pub fn from_sample_index(index: u64) -> Self {
        Self(index * 1000 / SAMPLE_RATE as u64)
    }
}

/// The one clock a Meeting's capture is measured against.
///
/// Monotonic on purpose: a wall-clock jump (NTP, daylight saving, the
/// Operator changing timezone mid-meeting) must not make the recording
/// appear to travel backwards.
#[derive(Debug, Clone)]
pub struct CaptureClock {
    started: std::time::Instant,
}

impl CaptureClock {
    pub fn start() -> Self {
        Self {
            started: std::time::Instant::now(),
        }
    }

    pub fn now(&self) -> CaptureOffset {
        CaptureOffset(self.started.elapsed().as_millis() as u64)
    }
}

impl Default for CaptureClock {
    fn default() -> Self {
        Self::start()
    }
}

/// One chunk of mono audio from one leg, stamped when captured.
#[derive(Debug, Clone)]
pub struct AudioFrame {
    pub channel: AudioChannel,
    /// When the *first* sample in this frame was captured.
    pub offset: CaptureOffset,
    pub samples: Vec<f32>,
}

impl AudioFrame {
    pub fn new(channel: AudioChannel, offset: CaptureOffset, samples: Vec<f32>) -> Self {
        Self {
            channel,
            offset,
            samples,
        }
    }

    pub fn duration_ms(&self) -> u64 {
        self.samples.len() as u64 * 1000 / SAMPLE_RATE as u64
    }

    /// Offset just past this frame's last sample.
    pub fn end_offset(&self) -> CaptureOffset {
        CaptureOffset(self.offset.millis() + self.duration_ms())
    }
}

/// What a capture source tells the supervisor.
#[derive(Debug)]
pub enum CaptureEvent {
    Frame(AudioFrame),
    /// The default device changed under us. Restarting this leg is expected
    /// housekeeping, not a failure, so it does not spend restart budget.
    DeviceChanged {
        channel: AudioChannel,
    },
    /// The stream died. Restarting spends budget; exhausting it ends the leg.
    StreamFailed {
        channel: AudioChannel,
        error: String,
    },
    /// This leg will produce nothing on this machine (for example system
    /// audio where the platform capture is unavailable). Reported once, and
    /// never retried — retrying an unsupported thing is just noise.
    Unavailable {
        channel: AudioChannel,
        reason: String,
    },
    /// This leg is delivering, but what it delivers is not usable, and here
    /// is why. Distinct from `Unavailable` because the leg is still
    /// attached: the reason reaches the record without the leg being ended,
    /// so a wrong diagnosis costs a sentence rather than the rest of the
    /// meeting's audio (DECISIONS Q9).
    Degraded {
        channel: AudioChannel,
        reason: String,
    },
}

/// The seam every test drives (PRD Testing Decisions).
///
/// Live capture and fixture playback are interchangeable at exactly this
/// point, which is why the churn contract, the sink, and the ASR pipeline
/// can all be tested without a microphone.
pub trait AudioSource: Send {
    /// Begins producing events. Called once per source instance; a restart
    /// creates a new instance rather than reusing this one.
    fn start(
        &mut self,
        clock: CaptureClock,
        events: tokio::sync::mpsc::Sender<CaptureEvent>,
    ) -> anyhow::Result<()>;

    /// Stops producing. Must be safe to call more than once.
    fn stop(&mut self);

    /// For logs and errors.
    fn describe(&self) -> String;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_and_sample_indices_agree() {
        assert_eq!(CaptureOffset(1000).sample_index(), 48_000);
        assert_eq!(
            CaptureOffset::from_sample_index(48_000),
            CaptureOffset(1000)
        );
        assert_eq!(CaptureOffset::ZERO.sample_index(), 0);
    }

    #[test]
    fn a_frames_duration_follows_its_sample_count() {
        let frame = AudioFrame::new(
            AudioChannel::Mic,
            CaptureOffset(100),
            vec![0.0; SAMPLES_PER_FRAME],
        );
        assert_eq!(frame.duration_ms(), FRAME_MS);
        assert_eq!(frame.end_offset(), CaptureOffset(100 + FRAME_MS));
    }

    #[test]
    fn the_clock_moves_forward() {
        let clock = CaptureClock::start();
        let first = clock.now();
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(clock.now() >= first);
    }
}
