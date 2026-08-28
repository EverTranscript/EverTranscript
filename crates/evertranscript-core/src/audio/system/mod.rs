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
#[cfg(target_os = "windows")]
pub use windows::available as windows_available;

/// A running system-audio capture. Dropping or stopping it releases the
/// platform resources — on macOS that includes a tap and an aggregate device
/// that would otherwise outlive the process.
pub trait SystemCapture: Send {
    fn stop(&mut self);
    fn describe(&self) -> String;
}

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
