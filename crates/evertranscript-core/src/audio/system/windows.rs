//! System audio on Windows: WASAPI loopback.
//!
//! Far less ceremony than the macOS side. cpal turns an input stream built
//! on an *output* device into a loopback capture, so the whole platform
//! difference is which device we hand it. No virtual device, no aggregate,
//! and no permission prompt — Windows does not gate loopback capture.
//!
//! The device is the default *output*: whatever the Operator is listening
//! to is, by definition, the far end of their meeting.

use anyhow::Context;
use anyhow::Result;
use cpal::traits::DeviceTrait;
use cpal::traits::HostTrait;
use cpal::traits::StreamTrait;
use evertranscript_protocol::AudioChannel;
use tokio::sync::mpsc;
use tracing::info;
use tracing::warn;

use super::SystemCapture;
use crate::audio::CaptureClock;
use crate::audio::CaptureEvent;
use crate::audio::leg::LegEncoder;

/// cpal streams are `!Send`, so the stream lives on its own thread and is
/// stopped by telling that thread to drop it — the same shape the
/// microphone leg uses.
pub struct LoopbackCapture {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    device: String,
}

impl SystemCapture for LoopbackCapture {
    fn stop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    fn describe(&self) -> String {
        format!("WASAPI loopback on {}", self.device)
    }
}

impl Drop for LoopbackCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn start(
    clock: CaptureClock,
    events: mpsc::Sender<CaptureEvent>,
) -> Result<Box<dyn SystemCapture>> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .context("this machine has no audio output, so there is nothing to record")?;
    let name = device.name().unwrap_or_else(|_| "unknown".to_string());
    // The *output* config: loopback captures what the device is playing, so
    // the format to match is its playback format, not an input one.
    let config = device
        .default_output_config()
        .context("the output device has no usable configuration")?;
    info!(device = %name, rate = config.sample_rate().0, channels = config.channels(), "system-audio loopback starting");

    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let thread_stop = std::sync::Arc::clone(&stop);
    let thread_clock = clock;
    let handle = std::thread::Builder::new()
        .name("evertranscript-loopback".to_string())
        .spawn(move || {
            if let Err(error) = run(device, config, thread_clock, events, thread_stop) {
                warn!(%error, "system-audio loopback ended");
            }
        })
        .context("spawning the loopback thread")?;

    Ok(Box::new(LoopbackCapture {
        stop,
        handle: Some(handle),
        device: name,
    }))
}

fn run(
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
    clock: CaptureClock,
    events: mpsc::Sender<CaptureEvent>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    let mut encoder = LegEncoder::new(
        AudioChannel::System,
        config.channels() as usize,
        config.sample_rate().0,
        clock,
    )?;
    let error_events = events.clone();

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config.clone().into(),
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                if let Some(frame) = encoder.encode(data) {
                    // Never block the audio thread; a dropped frame becomes
                    // an honest gap rather than a shifted timeline.
                    let _ = events.try_send(CaptureEvent::Frame(frame));
                }
            },
            move |error: cpal::StreamError| {
                let _ = error_events.try_send(CaptureEvent::StreamFailed {
                    channel: AudioChannel::System,
                    error: error.to_string(),
                });
            },
            None,
        )?,
        other => {
            anyhow::bail!("unsupported loopback sample format {other:?} — only f32 is handled")
        }
    };
    stream.play().context("starting the loopback stream")?;

    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    drop(stream);
    Ok(())
}

/// Whether loopback capture is possible. Windows needs no grant, so this
/// only asks whether an output device exists.
///
/// Asked of WASAPI directly rather than through cpal. `default_host()`
/// aborts the process on a machine with no audio device and the audio
/// service stopped — not a Rust panic, so nothing can catch it — and a
/// preflight that can kill the Core is worse than no preflight. Every step
/// here fails into a plain `Err`, which is what the caller already handles.
pub fn available() -> std::result::Result<(), String> {
    use windows::Win32::Media::Audio::IMMDeviceEnumerator;
    use windows::Win32::Media::Audio::MMDeviceEnumerator;
    use windows::Win32::Media::Audio::eMultimedia;
    use windows::Win32::Media::Audio::eRender;
    use windows::Win32::System::Com::CLSCTX_ALL;
    use windows::Win32::System::Com::COINIT_MULTITHREADED;
    use windows::Win32::System::Com::CoCreateInstance;
    use windows::Win32::System::Com::CoInitializeEx;

    const NO_OUTPUT: &str = "this machine has no audio output, so there is nothing to record";

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(enumerator) =
            CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
        else {
            return Err("this machine's audio system could not be asked".to_string());
        };
        match enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia) {
            Ok(_) => Ok(()),
            Err(_) => Err(NO_OUTPUT.to_string()),
        }
    }
}
