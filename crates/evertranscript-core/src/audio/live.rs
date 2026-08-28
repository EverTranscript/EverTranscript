//! Live capture from real hardware.
//!
//! The microphone leg is cpal on both platforms — the same binding anarlog
//! and Meetily ship. The system-audio leg is where the platforms diverge
//! (CoreAudio process taps on macOS, WASAPI loopback on Windows) and is not
//! implemented yet; it reports itself unavailable, which the churn policy
//! already handles as "record the microphone and say the audio is partial".
//! That degradation is tested, so the gap is visible rather than silent.

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

use super::AudioFrame;
use super::AudioSource;
use super::CaptureClock;
use super::CaptureEvent;
use super::CaptureOffset;
use super::SAMPLE_RATE;

/// Captures the default microphone, and reports system audio as unavailable.
pub struct LiveSource {
    stream: Option<Box<dyn StreamHandle>>,
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
        Self { stream: None }
    }

    /// True when a default input device exists. Used to decide whether live
    /// capture is even possible before a Meeting starts.
    pub fn microphone_available() -> bool {
        cpal::default_host().default_input_device().is_some()
    }
}

impl AudioSource for LiveSource {
    fn start(&mut self, clock: CaptureClock, events: mpsc::Sender<CaptureEvent>) -> Result<()> {
        // System audio: the platform work (process taps / WASAPI loopback)
        // is not done. Saying so once is better than pretending to capture
        // silence — the joiner stops waiting on the leg and the Meeting is
        // marked partial.
        let unavailable = events.try_send(CaptureEvent::Unavailable {
            channel: AudioChannel::System,
            reason: system_audio_reason().to_string(),
        });
        if unavailable.is_err() {
            debug!("nobody is listening for capture events yet");
        }

        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .context("no default input device — is a microphone connected?")?;
        let name = device.name().unwrap_or_else(|_| "unknown".to_string());
        let config = device
            .default_input_config()
            .context("the input device has no usable configuration")?;
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
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(mut stream) = self.stream.take() {
            stream.stop();
        }
    }

    fn describe(&self) -> String {
        "live (microphone)".to_string()
    }
}

fn system_audio_reason() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "system-audio capture (CoreAudio process taps) is not implemented yet"
    }
    #[cfg(target_os = "windows")]
    {
        "system-audio capture (WASAPI loopback) is not implemented yet"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "system-audio capture is not available on this platform"
    }
}

fn run_microphone(
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
    clock: CaptureClock,
    events: mpsc::Sender<CaptureEvent>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    let input_channels = config.channels() as usize;
    let input_rate = config.sample_rate().0;
    let error_events = events.clone();

    let data_callback = move |data: &[f32], _: &cpal::InputCallbackInfo| {
        // Downmix to mono and resample to the capture rate. Naive
        // nearest-sample resampling is a placeholder: ticket 08 replaces it
        // with the persistent sinc resampler the DSP work needs.
        let frames = data.len() / input_channels.max(1);
        let mut mono = Vec::with_capacity(frames);
        for frame in data.chunks_exact(input_channels.max(1)) {
            mono.push(frame.iter().sum::<f32>() / input_channels as f32);
        }
        let samples = if input_rate == SAMPLE_RATE {
            mono
        } else {
            resample_nearest(&mono, input_rate, SAMPLE_RATE)
        };
        if samples.is_empty() {
            return;
        }

        // The frame is stamped for where it *starts*, which is now minus its
        // own duration: the samples in hand were captured before the
        // callback ran, and stamping them at "now" would push the whole
        // timeline late by one buffer.
        let duration_ms = samples.len() as u64 * 1000 / SAMPLE_RATE as u64;
        let offset = CaptureOffset(clock.now().millis().saturating_sub(duration_ms));

        // Never block the audio thread: a full queue means the consumer is
        // behind, and dropping a frame is far better than glitching capture.
        // The joiner turns the dropped span into silence, so the timeline
        // stays honest.
        let _ = events.try_send(CaptureEvent::Frame(AudioFrame::new(
            AudioChannel::Mic,
            offset,
            samples,
        )));
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

/// Nearest-sample resampling. Adequate to get bytes flowing; ticket 08
/// replaces it with a persistent sinc resampler (per-chunk construction is a
/// known source of amplitude drift).
fn resample_nearest(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if input.is_empty() || from_rate == 0 {
        return Vec::new();
    }
    let out_len = (input.len() as u64 * to_rate as u64 / from_rate as u64) as usize;
    (0..out_len)
        .map(|index| {
            let source = index as u64 * from_rate as u64 / to_rate as u64;
            input[(source as usize).min(input.len() - 1)]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampling_preserves_length_proportionally() {
        let input = vec![0.5; 480]; // 10 ms at 48 kHz
        let out = resample_nearest(&input, 48_000, 16_000);
        assert_eq!(out.len(), 160, "10 ms at 16 kHz");
        assert!(out.iter().all(|sample| *sample == 0.5));

        let up = resample_nearest(&input, 24_000, 48_000);
        assert_eq!(up.len(), 960);
    }

    #[test]
    fn resampling_an_empty_buffer_is_not_a_panic() {
        assert!(resample_nearest(&[], 48_000, 16_000).is_empty());
        assert!(resample_nearest(&[1.0], 0, 16_000).is_empty());
    }

    #[tokio::test]
    async fn a_live_source_reports_system_audio_as_unavailable() {
        // The microphone half needs hardware and a TCC grant, so this test
        // covers what is deterministic: the system leg announces itself
        // unavailable, which is what keeps the recording from stalling on a
        // leg that will never deliver.
        let (events_tx, mut events_rx) = mpsc::channel(8);
        let mut source = LiveSource::new();
        // Starting may fail without a microphone; the system-audio event is
        // sent first either way.
        let _ = source.start(CaptureClock::start(), events_tx);
        source.stop();

        let first = events_rx.try_recv().expect("an event");
        match first {
            CaptureEvent::Unavailable { channel, reason } => {
                assert_eq!(channel, AudioChannel::System);
                assert!(reason.contains("not implemented") || reason.contains("not available"));
            }
            other => panic!("expected the system leg to report unavailable, got {other:?}"),
        }
    }
}
