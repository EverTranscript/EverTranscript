//! Pairs the two capture legs into one stereo timeline.
//!
//! This is where the desync both open-source competitors ship gets fixed.
//! When a leg stalls — a device swap, a stream restart, a system-audio
//! hiccup — they simply write fewer samples, so the audio file ends up
//! *shorter than wall clock* while transcript timestamps stay wall-clock
//! anchored. The two drift apart by exactly the outage, silently, forever.
//!
//! Here, every output sample has a position on the capture clock and gaps
//! are filled with silence. The file length always equals elapsed time, so
//! a timestamp means the same thing in the transcript and in the audio no
//! matter what the hardware did.

use evertranscript_protocol::AudioChannel;

use super::AudioFrame;
use super::CaptureOffset;
use super::SAMPLE_RATE;

/// One interleaved stereo block: left = mic, right = system (ADR-0032).
#[derive(Debug, Clone, PartialEq)]
pub struct StereoBlock {
    /// Where this block starts on the capture clock.
    pub offset: CaptureOffset,
    /// Interleaved `[mic, system, mic, system, …]`.
    pub samples: Vec<f32>,
}

impl StereoBlock {
    pub fn frame_count(&self) -> usize {
        self.samples.len() / 2
    }
}

/// How far ahead of the emitted timeline a leg may run before the other is
/// declared late and filled with silence.
///
/// The two legs are separate hardware clocks; they never arrive in lockstep.
/// Too small and normal jitter produces silence holes; too large and a dead
/// leg delays the recording. 400 ms is comfortably past scheduling jitter
/// and still imperceptible.
const MAX_LEAD_MS: u64 = 400;

/// Buffers one leg's samples against the shared timeline.
///
/// The buffer is kept *contiguous from `pending_at`*: a frame arriving after
/// a dropout has silence inserted ahead of it rather than being concatenated
/// onto the previous frame. Concatenating is the subtle bug — it looks
/// correct and quietly slides every later sample earlier by the length of
/// the outage, which is exactly the drift this whole module exists to stop.
#[derive(Debug, Default)]
struct Leg {
    /// Samples not yet emitted, contiguous from `pending_at`.
    pending: Vec<f32>,
    pending_at: u64,
    /// Highest offset this leg has delivered anything for.
    delivered_to: u64,
    /// Everything before this has already been emitted.
    consumed_to: u64,
    /// True once the leg reports it will never produce (unsupported, or
    /// permanently failed). Its side is silence from then on, with no wait.
    finished: bool,
}

impl Leg {
    fn push(&mut self, frame: &AudioFrame) {
        let start = frame.offset.millis();
        let end = frame.end_offset().millis();
        self.delivered_to = self.delivered_to.max(end);

        // Entirely in the past: its slot has already been written. Dropping
        // it costs one frame; splicing it in would shift everything after.
        if end <= self.consumed_to {
            return;
        }

        // Partly in the past: keep only the part that still has a slot.
        let (start, samples) = if start < self.consumed_to {
            let skip = ms_to_samples(self.consumed_to - start).min(frame.samples.len());
            (self.consumed_to, &frame.samples[skip..])
        } else {
            (start, &frame.samples[..])
        };

        if self.pending.is_empty() {
            self.pending_at = start;
        } else {
            let pending_end = self.pending_at + samples_to_ms(self.pending.len());
            if start > pending_end {
                // The dropout, preserved as silence.
                let hole = ms_to_samples(start - pending_end);
                self.pending.extend(std::iter::repeat_n(0.0, hole));
            }
        }
        self.pending.extend_from_slice(samples);
    }

    /// Samples for the window starting at `from`, padded with silence for
    /// any part of it this leg did not deliver.
    fn take(&mut self, from: u64, sample_count: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(sample_count);

        if !self.pending.is_empty() {
            if self.pending_at > from {
                let hole = ms_to_samples(self.pending_at - from).min(sample_count);
                out.extend(std::iter::repeat_n(0.0, hole));
            }
            let wanted = sample_count.saturating_sub(out.len());
            let available = wanted.min(self.pending.len());
            out.extend(self.pending.drain(..available));
            self.pending_at += samples_to_ms(available);
        }

        // Silence for whatever the leg still owes.
        out.resize(sample_count, 0.0);
        self.consumed_to = from + samples_to_ms(sample_count);
        out
    }
}

fn ms_to_samples(ms: u64) -> usize {
    (ms * SAMPLE_RATE as u64 / 1000) as usize
}

fn samples_to_ms(samples: usize) -> u64 {
    samples as u64 * 1000 / SAMPLE_RATE as u64
}

/// Turns two independent streams of mono frames into one gap-free stereo
/// timeline.
#[derive(Debug, Default)]
pub struct Joiner {
    mic: Leg,
    system: Leg,
    /// Everything before this offset has already been emitted.
    emitted_to: u64,
}

impl Joiner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, frame: &AudioFrame) {
        match frame.channel {
            AudioChannel::Mic => self.mic.push(frame),
            AudioChannel::System => self.system.push(frame),
        }
    }

    /// Marks a leg as producing nothing further, so the joiner stops waiting
    /// on it and fills its side with silence.
    pub fn finish_leg(&mut self, channel: AudioChannel) {
        match channel {
            AudioChannel::Mic => self.mic.finished = true,
            AudioChannel::System => self.system.finished = true,
        }
    }

    /// Emits every stereo block that is now safe to write.
    ///
    /// A block is safe once both legs have delivered past its end, or once
    /// one leg has run `MAX_LEAD_MS` ahead of the other (the other is late,
    /// and waiting longer would stall the recording), or once a leg is
    /// finished and can never deliver.
    pub fn drain(&mut self) -> Vec<StereoBlock> {
        let mut blocks = Vec::new();
        while let Some(block) = self.next_block() {
            blocks.push(block);
        }
        blocks
    }

    fn next_block(&mut self) -> Option<StereoBlock> {
        // Only legs that can still deliver get a say in how far it is safe
        // to emit. A finished leg is silence, and waiting on it would stall
        // the recording forever.
        let safe_to = match (!self.mic.finished, !self.system.finished) {
            (true, true) => {
                // Normally emit up to where *both* have delivered; but a leg
                // running far ahead means the other is late rather than
                // coming, so the leader's margin forces progress.
                let both = self.mic.delivered_to.min(self.system.delivered_to);
                let leader = self.mic.delivered_to.max(self.system.delivered_to);
                both.max(leader.saturating_sub(MAX_LEAD_MS))
            }
            (true, false) => self.mic.delivered_to,
            (false, true) => self.system.delivered_to,
            (false, false) => return None,
        };

        if safe_to <= self.emitted_to {
            return None;
        }
        let span_ms = safe_to - self.emitted_to;
        let sample_count = ms_to_samples(span_ms);
        if sample_count == 0 {
            return None;
        }

        let mic = self.mic.take(self.emitted_to, sample_count);
        let system = self.system.take(self.emitted_to, sample_count);

        let mut interleaved = Vec::with_capacity(sample_count * 2);
        for index in 0..sample_count {
            interleaved.push(mic[index]);
            interleaved.push(system[index]);
        }

        let block = StereoBlock {
            offset: CaptureOffset(self.emitted_to),
            samples: interleaved,
        };
        self.emitted_to = safe_to;
        Some(block)
    }

    /// Flushes whatever remains at the end of a Meeting, padding both legs.
    pub fn flush(&mut self) -> Option<StereoBlock> {
        self.mic.finished = true;
        self.system.finished = true;
        let end = self.mic.delivered_to.max(self.system.delivered_to);
        if end <= self.emitted_to {
            return None;
        }
        let sample_count = ms_to_samples(end - self.emitted_to);
        if sample_count == 0 {
            return None;
        }
        let mic = self.mic.take(self.emitted_to, sample_count);
        let system = self.system.take(self.emitted_to, sample_count);
        let mut interleaved = Vec::with_capacity(sample_count * 2);
        for index in 0..sample_count {
            interleaved.push(mic[index]);
            interleaved.push(system[index]);
        }
        let block = StereoBlock {
            offset: CaptureOffset(self.emitted_to),
            samples: interleaved,
        };
        self.emitted_to = end;
        Some(block)
    }

    /// How much audio has been emitted, in milliseconds.
    pub fn emitted_ms(&self) -> u64 {
        self.emitted_to
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(channel: AudioChannel, offset_ms: u64, ms: u64, value: f32) -> AudioFrame {
        AudioFrame::new(
            channel,
            CaptureOffset(offset_ms),
            vec![value; ms_to_samples(ms)],
        )
    }

    /// Deinterleaves for readable assertions.
    fn split(blocks: &[StereoBlock]) -> (Vec<f32>, Vec<f32>) {
        let mut mic = Vec::new();
        let mut system = Vec::new();
        for block in blocks {
            for [left, right] in block.samples.as_chunks::<2>().0 {
                mic.push(*left);
                system.push(*right);
            }
        }
        (mic, system)
    }

    #[test]
    fn both_legs_arriving_together_produce_aligned_stereo() {
        let mut joiner = Joiner::new();
        joiner.push(&frame(AudioChannel::Mic, 0, 100, 0.5));
        joiner.push(&frame(AudioChannel::System, 0, 100, -0.5));

        let blocks = joiner.drain();
        let (mic, system) = split(&blocks);
        assert_eq!(mic.len(), ms_to_samples(100));
        assert_eq!(system.len(), ms_to_samples(100));
        assert!(mic.iter().all(|sample| *sample == 0.5));
        assert!(system.iter().all(|sample| *sample == -0.5));
    }

    #[test]
    fn a_gap_in_one_leg_becomes_silence_not_lost_time() {
        // The device-swap case: the mic leg goes away for 200 ms and comes
        // back. The output must still be 500 ms long, with silence where the
        // mic was missing — otherwise audio and transcript drift apart.
        let mut joiner = Joiner::new();
        joiner.push(&frame(AudioChannel::Mic, 0, 100, 1.0));
        joiner.push(&frame(AudioChannel::System, 0, 500, -1.0));
        // Mic resumes at 300 ms, after a 200 ms hole.
        joiner.push(&frame(AudioChannel::Mic, 300, 200, 1.0));

        let mut blocks = joiner.drain();
        blocks.extend(joiner.flush());
        let (mic, system) = split(&blocks);

        assert_eq!(
            mic.len(),
            ms_to_samples(500),
            "the timeline must stay wall-clock length through a dropout"
        );
        assert_eq!(system.len(), ms_to_samples(500));

        // The hole is silent, and the audio after it is back in position.
        let hole_start = ms_to_samples(100);
        let hole_end = ms_to_samples(300);
        assert!(
            mic[hole_start..hole_end]
                .iter()
                .all(|sample| *sample == 0.0),
            "the outage must be silence"
        );
        assert!(
            mic[hole_end..].iter().all(|sample| *sample == 1.0),
            "audio after the gap must land at its real timestamp, not slide earlier"
        );
    }

    #[test]
    fn one_leg_running_far_ahead_does_not_stall_the_recording() {
        // The system leg keeps flowing while the mic leg is being restarted.
        // Waiting for the mic would freeze the recording; instead the mic
        // side becomes silence and the meeting keeps being written.
        let mut joiner = Joiner::new();
        joiner.push(&frame(AudioChannel::System, 0, 2000, 0.25));

        let blocks = joiner.drain();
        assert!(
            !blocks.is_empty(),
            "a leading leg past the lead margin must be emitted"
        );
        let (mic, system) = split(&blocks);
        assert!(mic.iter().all(|sample| *sample == 0.0));
        assert!(system.iter().all(|sample| *sample == 0.25));
        assert!(
            joiner.emitted_ms() >= 2000 - MAX_LEAD_MS,
            "emission should track the leader minus the lead margin"
        );
    }

    #[test]
    fn a_finished_leg_stops_holding_the_timeline_back() {
        // System audio unavailable on this machine: the mic recording must
        // proceed immediately rather than waiting on a leg that will never
        // deliver.
        let mut joiner = Joiner::new();
        joiner.finish_leg(AudioChannel::System);
        joiner.push(&frame(AudioChannel::Mic, 0, 60, 0.75));

        let blocks = joiner.drain();
        let (mic, system) = split(&blocks);
        assert_eq!(mic.len(), ms_to_samples(60));
        assert!(mic.iter().all(|sample| *sample == 0.75));
        assert!(
            system.iter().all(|sample| *sample == 0.0),
            "the absent leg is silence"
        );
    }

    #[test]
    fn blocks_are_contiguous_and_never_overlap() {
        let mut joiner = Joiner::new();
        for step in 0..10 {
            joiner.push(&frame(AudioChannel::Mic, step * 20, 20, 1.0));
            joiner.push(&frame(AudioChannel::System, step * 20, 20, 1.0));
        }
        let mut blocks = joiner.drain();
        blocks.extend(joiner.flush());

        let mut expected_offset = 0;
        for block in &blocks {
            assert_eq!(
                block.offset.millis(),
                expected_offset,
                "blocks must tile the timeline with no overlap or hole"
            );
            expected_offset += samples_to_ms(block.frame_count());
        }
        assert_eq!(expected_offset, 200);
    }

    #[test]
    fn late_arriving_audio_does_not_shift_everything_after_it() {
        // A frame that shows up after its slot was already written cannot be
        // inserted without pushing later audio out of position, so it is
        // dropped. Losing 20 ms beats desyncing the rest of the meeting.
        let mut joiner = Joiner::new();
        joiner.push(&frame(AudioChannel::Mic, 0, 100, 1.0));
        joiner.push(&frame(AudioChannel::System, 0, 100, 1.0));
        let first = joiner.drain();
        assert!(!first.is_empty());

        joiner.push(&frame(AudioChannel::Mic, 0, 20, 0.1)); // too late
        joiner.push(&frame(AudioChannel::Mic, 100, 100, 1.0));
        joiner.push(&frame(AudioChannel::System, 100, 100, 1.0));
        let second = joiner.drain();

        let (mic, _) = split(&second);
        assert!(
            mic.iter().all(|sample| *sample == 1.0),
            "the late frame must not be spliced into the middle of the timeline"
        );
    }
}
