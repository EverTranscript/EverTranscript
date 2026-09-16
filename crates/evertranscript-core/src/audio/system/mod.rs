//! The system-audio leg: what the other participants said.
//!
//! This is the half of a meeting the microphone cannot hear. Without it a
//! recording is one side of a conversation, so the leg is not optional in
//! any real sense — but it is the part of capture that is most different
//! between platforms, and the part most likely to be refused at runtime.
//!
//! macOS takes a CoreAudio process tap (14.4+). That choice is ADR-0027 and
//! it is deliberate: the obvious alternative, ScreenCaptureKit, would work
//! but demands the Screen Recording permission — a grant that lets an app
//! read every window on the machine — to obtain audio that the narrower
//! audio-capture permission already covers. Asking for the larger power
//! would undercut the product's whole claim, so the guarantee suite fails
//! the build if ScreenCaptureKit is ever linked.
//!
//! Windows takes WASAPI loopback, which cpal exposes by building an *input*
//! stream on an *output* device.
//!
//! Failure here is ordinary, not exceptional: the permission may be
//! unresolved, the OS may be too old, the machine may have no output device.
//! Every one of those returns an error, the caller reports the leg
//! `Unavailable`, and the Meeting records the microphone and says its audio
//! is partial. A missing far end is a degraded recording; it is never a lost
//! one.

use anyhow::Result;
use tokio::sync::mpsc;

use super::CaptureClock;
use super::CaptureEvent;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::available as macos_available;

/// Whether anything on this machine is playing audio right now.
///
/// `None` means this platform cannot say — which callers must read as
/// "cannot tell", never as "nothing is playing". The distinction is the
/// whole point: silence with nothing playing is an ordinary quiet meeting,
/// while silence with something playing is a refused permission
/// (DECISIONS Q9).
pub fn output_is_active() -> Option<bool> {
    #[cfg(target_os = "macos")]
    {
        Some(macos::anything_is_playing())
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Whether the machine's default output goes into someone's ears rather
/// than into the room.
///
/// The question behind it is not about hardware. Headphones mean the far
/// end of the meeting could not have reached the microphone, so every voice
/// on the mic channel is the Operator — ADR-0029's first rule, which names
/// "You" with no act at all. That is a strong conclusion to draw, so the
/// failure that costs the most is the *false* `true`: a speaker in a
/// conference room reported as headphones would give a colleague's words
/// the Operator's name, invisibly.
///
/// `None` means this platform, or this device, cannot say. Callers read it
/// as "cannot tell" and never as "no" — and rule 1 fires only on
/// `Some(true)`, so an unclassifiable device simply falls through to the
/// rules that look at the audio.
pub fn output_is_headphones() -> Option<bool> {
    #[cfg(target_os = "macos")]
    {
        macos::output_is_headphones()
    }
    #[cfg(target_os = "windows")]
    {
        windows::output_is_headphones()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

/// Whether the far end could have reached the microphone, accumulated over
/// one Meeting.
///
/// A fact about the recording that only the recording can establish, which
/// is why it is gathered here and written to the Meeting at stop: by the
/// time diarization runs, the audio no longer says what was plugged in.
///
/// Two things have to hold, and both of them all the way through. The
/// output must have gone into someone's ears every time it was looked at,
/// and the **microphone must not have been swapped** — a Meeting that began
/// on a headset and continued on the laptop's own microphone is exactly the
/// case where the far end reaches it, and nothing later in the pipeline
/// could tell.
///
/// Anything short of certain is [`None`], never `Some(false)`: `Some(false)`
/// is itself a claim, that the room was audible, and the caller who reads
/// this one day deserves to know which of the two it got.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MicIsolation {
    saw_headphones: bool,
    saw_room: bool,
    saw_unknown: bool,
    mic_swapped: bool,
}

impl MicIsolation {
    /// Records one reading of the output device, as
    /// [`output_is_headphones`] answers it. Takes the reading rather than
    /// making it, so the rules below are testable without hardware.
    pub fn observe(&mut self, headphones: Option<bool>) {
        match headphones {
            Some(true) => self.saw_headphones = true,
            Some(false) => self.saw_room = true,
            None => self.saw_unknown = true,
        }
    }

    /// Records a capture event, watching for the microphone changing under
    /// the Meeting.
    pub fn note(&mut self, event: &CaptureEvent) {
        if let CaptureEvent::DeviceChanged {
            channel: evertranscript_protocol::AudioChannel::Mic,
        } = event
        {
            self.mic_swapped = true;
        }
    }

    /// The Meeting's answer, in the three states the column holds.
    pub fn verdict(&self) -> Option<bool> {
        if self.mic_swapped || self.saw_room {
            return Some(false);
        }
        if self.saw_unknown || !self.saw_headphones {
            return None;
        }
        Some(true)
    }
}

#[cfg(target_os = "windows")]
pub use windows::available as windows_available;

/// Starts system-audio capture, feeding mono frames at the capture rate into
/// `events`.
///
/// The error is shown to the Operator as the reason the leg is unavailable,
/// so it must say what is wrong in terms they can act on.
pub fn start(
    clock: CaptureClock,
    events: mpsc::Sender<CaptureEvent>,
) -> Result<Box<dyn SystemCapture>> {
    #[cfg(target_os = "macos")]
    {
        macos::start(clock, events)
    }
    #[cfg(target_os = "windows")]
    {
        windows::start(clock, events)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (clock, events);
        anyhow::bail!("system-audio capture is not implemented on this platform")
    }
}

/// A running system-audio capture. Dropping or stopping it releases the
/// platform resources — on macOS that includes a tap and an aggregate device
/// that would otherwise outlive the process.
pub trait SystemCapture: Send {
    fn stop(&mut self);
    fn describe(&self) -> String;
}

#[cfg(test)]
mod tests {
    use super::*;
    use evertranscript_protocol::AudioChannel;

    #[test]
    fn headphones_throughout_and_one_microphone_is_an_isolated_meeting() {
        let mut isolation = MicIsolation::default();
        for _ in 0..12 {
            isolation.observe(Some(true));
        }
        assert_eq!(isolation.verdict(), Some(true));
    }

    #[test]
    fn a_meeting_nobody_looked_at_says_nothing_rather_than_no() {
        // Every Meeting recorded before this existed, and every platform
        // that cannot answer. `None` is what makes those readable as
        // "never asked" instead of as "the room was audible".
        assert_eq!(MicIsolation::default().verdict(), None);

        let mut unknown = MicIsolation::default();
        unknown.observe(None);
        assert_eq!(unknown.verdict(), None, "a USB device could be either");
    }

    #[test]
    fn one_glance_at_the_room_settles_it_for_the_whole_meeting() {
        // Headphones for fifty-nine minutes and the speaker for one is a
        // Meeting where the far end reached the microphone. The rule that
        // matters is "every time", not "mostly".
        let mut isolation = MicIsolation::default();
        for _ in 0..59 {
            isolation.observe(Some(true));
        }
        isolation.observe(Some(false));
        assert_eq!(isolation.verdict(), Some(false));
    }

    #[test]
    fn an_unclassifiable_stretch_withdraws_the_claim_without_making_another() {
        let mut isolation = MicIsolation::default();
        isolation.observe(Some(true));
        isolation.observe(None);
        assert_eq!(
            isolation.verdict(),
            None,
            "part of this Meeting is unaccounted for, which is not the same as the room"
        );
    }

    #[test]
    fn swapping_the_microphone_ends_the_guarantee() {
        // The case the transport probe alone cannot see: headphones the
        // whole way through, and halfway in the Operator unplugs the
        // headset and the laptop's own microphone takes over. It now hears
        // the room, and nothing downstream could tell.
        let mut isolation = MicIsolation::default();
        isolation.observe(Some(true));
        isolation.note(&CaptureEvent::DeviceChanged {
            channel: AudioChannel::Mic,
        });
        isolation.observe(Some(true));
        assert_eq!(isolation.verdict(), Some(false));
    }

    #[test]
    fn swapping_the_far_ends_device_is_not_the_microphone_changing() {
        // The system leg restarting is ordinary churn — it is the *output*
        // moving, which the next `observe` reads for itself.
        let mut isolation = MicIsolation::default();
        isolation.observe(Some(true));
        isolation.note(&CaptureEvent::DeviceChanged {
            channel: AudioChannel::System,
        });
        assert_eq!(isolation.verdict(), Some(true));
    }
}

