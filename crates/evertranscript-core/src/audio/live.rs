//! Live capture from real hardware.
//!
//! Two independent legs. The microphone is cpal on both platforms. System
//! audio — the other participants — is a CoreAudio process tap on macOS and
//! WASAPI loopback on Windows, both behind [`super::system`].
//!
//! The legs are deliberately not tied together. A machine with no output
//! device, an Operator who has not granted audio-capture permission, or a
//! macOS older than 14.4 all produce a Meeting that records the microphone
//! and says its audio is partial. The reverse holds too: a missing
//! microphone does not stop system audio. Losing half a conversation is bad;
//! losing the meeting because half was unavailable would be worse.

use anyhow::Context;
use anyhow::Result;
use cpal::traits::DeviceTrait;
use cpal::traits::HostTrait;
use cpal::traits::StreamTrait;
use evertranscript_protocol::AudioChannel;
use tokio::sync::mpsc;
use tracing::debug;
use tracing::info;
use tracing::warn;

use super::leg::LegEncoder;
use super::system;
use super::AudioSource;
use super::CaptureClock;
use super::CaptureEvent;

/// Captures the microphone and, where the platform allows it, system audio.
pub struct LiveSource {
    stream: Option<Box<dyn StreamHandle>>,
    system: Option<Box<dyn system::SystemCapture>>,
    description: String,
}

/// cpal streams are not `Send`, so they live on their own thread and are
/// stopped by dropping them there.
trait StreamHandle: Send {
    fn stop(&mut self);
}

struct ThreadStream {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl StreamHandle for ThreadStream {
    fn stop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Default for LiveSource {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveSource {
    pub fn new() -> Self {
        Self {
            stream: None,
            system: None,
            description: "live".to_string(),
        }
    }

    /// True when a default input device exists. Used to decide whether live
    /// capture is even possible before a Meeting starts.
    pub fn microphone_available() -> bool {
        cpal::default_host().default_input_device().is_some()
    }

    /// Starts the microphone leg, returning the device's name.
    fn start_microphone(
        &mut self,
        clock: CaptureClock,
        events: mpsc::Sender<CaptureEvent>,
    ) -> Result<String> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .context("no default input device — is a microphone connected?")?;
        let name = device.name().unwrap_or_else(|_| "unknown".to_string());
        // A device can exist and still be unusable: a machine with no built-in
        // microphone reports one and then refuses to describe it.
        let config = device
            .default_input_config()
            .with_context(|| format!("the input device \"{name}\" has no usable configuration"))?;
        info!(device = %name, rate = config.sample_rate().0, channels = config.channels(), "microphone capture starting");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_stop = std::sync::Arc::clone(&stop);

        // cpal's Stream is !Send, so it is created and dropped on one
        // dedicated thread rather than moved into the async runtime.
        let handle = std::thread::Builder::new()
            .name("evertranscript-mic".to_string())
            .spawn(move || {
                if let Err(error) = run_microphone(device, config, clock, events, thread_stop) {
                    warn!(%error, "microphone capture ended");
                }
            })
            .context("spawning the microphone thread")?;

        self.stream = Some(Box::new(ThreadStream {
            stop,
            handle: Some(handle),
        }));
        Ok(name)
    }

    /// Whether system audio can be captured on this machine, and why not if
    /// it cannot. Asks the platform rather than guessing from the OS version,
    /// because the usual answer is an ungranted permission.
    pub fn system_audio_available() -> std::result::Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            system::macos_available()
        }
        #[cfg(target_os = "windows")]
        {
            system::windows_available()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            Err("system-audio capture is not implemented on this platform".to_string())
        }
    }
}

impl AudioSource for LiveSource {
    fn start(&mut self, clock: CaptureClock, events: mpsc::Sender<CaptureEvent>) -> Result<()> {
        // Both legs are attempted, and neither can veto the other. Starting
        // system audio first only fixes the order of the log lines; what
        // matters is that a failure below records the leg as unavailable and
        // keeps going.
        let system = match system::start(clock.clone(), events.clone()) {
            Ok(capture) => {
                info!(via = %capture.describe(), "system audio joined the recording");
                self.system = Some(capture);
                None
            }
            Err(error) => {
                warn!(%error, "recording without system audio");
                Some(format!("{error:#}"))
            }
        };

        let microphone = match self.start_microphone(clock, events.clone()) {
            Ok(name) => {
                self.description = match &self.system {
                    Some(capture) => format!("live ({name} + {})", capture.describe()),
                    None => format!("live ({name}, microphone only)"),
                };
                None
            }
            Err(error) => {
                warn!(%error, "recording without a microphone");
                self.description = match &self.system {
                    Some(capture) => format!("live ({}, system audio only)", capture.describe()),
                    None => "live (nothing available)".to_string(),
                };
                Some(format!("{error:#}"))
            }
        };

        // Nothing to record is the one case that is genuinely an error: a
        // Meeting with no audio at all is not a degraded recording, it is
        // the absence of one.
        if let (Some(system), Some(microphone)) = (&system, &microphone) {
            anyhow::bail!("no audio can be captured — microphone: {microphone}; system: {system}");
        }

        // Each missing leg is announced exactly once. The joiner waits on
        // legs it has not been told about, so an unannounced leg would stall
        // the recording; and retrying something the platform has refused is
        // noise rather than resilience.
        for (channel, reason) in [
            (AudioChannel::System, system),
            (AudioChannel::Mic, microphone),
        ] {
            if let Some(reason) = reason {
                if events
                    .try_send(CaptureEvent::Unavailable { channel, reason })
                    .is_err()
                {
                    debug!("nobody is listening for capture events yet");
                }
            }
        }
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(mut stream) = self.stream.take() {
            stream.stop();
        }
        if let Some(mut capture) = self.system.take() {
            capture.stop();
        }
    }

    fn describe(&self) -> String {
        self.description.clone()
    }
}

fn run_microphone(
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
    clock: CaptureClock,
    events: mpsc::Sender<CaptureEvent>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    let mut encoder = LegEncoder::new(
        AudioChannel::Mic,
        config.channels() as usize,
        config.sample_rate().0,
        clock,
    )?;
    let error_events = events.clone();

    let data_callback = move |data: &[f32], _: &cpal::InputCallbackInfo| {
        let Some(frame) = encoder.encode(data) else {
            return;
        };
        // Never block the audio thread: a full queue means the consumer is
        // behind, and dropping a frame is far better than glitching capture.
        // The joiner turns the dropped span into silence, so the timeline
        // stays honest.
        let _ = events.try_send(CaptureEvent::Frame(frame));
    };

    let error_callback = move |error: cpal::StreamError| {
        let _ = error_events.try_send(CaptureEvent::StreamFailed {
            channel: AudioChannel::Mic,
            error: error.to_string(),
        });
    };

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config.clone().into(),
            data_callback,
            error_callback,
            None,
        )?,
        other => anyhow::bail!(
            "unsupported microphone sample format {other:?} — only f32 is handled so far"
        ),
    };
    stream.play().context("starting the microphone stream")?;

    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    drop(stream);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asking_about_system_audio_answers_rather_than_panicking() {
        // Whatever this machine's answer is, it must be a stated one: the
        // preflight shows it to the Operator, and "unknown" is not a thing
        // they can act on.
        match LiveSource::system_audio_available() {
            Ok(()) => {}
            Err(reason) => assert!(
                !reason.is_empty(),
                "an unavailable leg must say why it is unavailable"
            ),
        }
    }

    #[tokio::test]
    async fn a_leg_that_cannot_start_is_announced_rather_than_left_silent() {
        // The joiner waits on legs it has not been told about, so whichever
        // leg is missing on this machine must be named before any frame
        // arrives — otherwise the recording stalls on audio that never comes.
        let (events_tx, mut events_rx) = mpsc::channel(8);
        let mut source = LiveSource::new();
        let started = source.start(CaptureClock::start(), events_tx);
        source.stop();

        let mut announced = Vec::new();
        while let Ok(event) = events_rx.try_recv() {
            if let CaptureEvent::Unavailable { channel, reason } = event {
                assert!(!reason.is_empty(), "an unavailable leg must say why");
                announced.push(channel);
            }
        }

        match started {
            Ok(()) => assert!(
                announced.len() < 2,
                "a source that started cannot have lost both legs"
            ),
            // Both legs down is the one honest failure: a Meeting with no
            // audio at all is not a degraded recording, it is no recording.
            Err(error) => {
                let error = format!("{error:#}");
                assert!(
                    error.contains("microphone") && error.contains("system"),
                    "failing to record must name both legs, got {error}"
                );
            }
        }
    }

    #[tokio::test]
    async fn one_leg_failing_does_not_take_the_other_down_with_it() {
        // The whole point of independent legs. This machine has exactly one
        // working leg, which makes it the case worth asserting: a recording
        // still starts, and the missing half is reported rather than fatal.
        let microphone = {
            let (tx, _rx) = mpsc::channel(4);
            let mut probe = LiveSource::new();
            let started = probe.start_microphone(CaptureClock::start(), tx).is_ok();
            probe.stop();
            started
        };
        let system = LiveSource::system_audio_available().is_ok();
        if !(microphone ^ system) {
            // Both legs up or both down: nothing to prove here.
            return;
        }

        let (events_tx, mut events_rx) = mpsc::channel(64);
        let mut source = LiveSource::new();
        source
            .start(CaptureClock::start(), events_tx)
            .expect("one working leg is enough to record");
        let description = source.describe();
        source.stop();

        let mut unavailable = Vec::new();
        while let Ok(event) = events_rx.try_recv() {
            if let CaptureEvent::Unavailable { channel, .. } = event {
                unavailable.push(channel);
            }
        }
        let missing = if microphone {
            AudioChannel::System
        } else {
            AudioChannel::Mic
        };
        assert_eq!(
            unavailable,
            vec![missing],
            "exactly the missing leg should be reported unavailable"
        );
        assert!(
            description.contains("only"),
            "a half-deaf recording should say so, got {description:?}"
        );
    }
}
