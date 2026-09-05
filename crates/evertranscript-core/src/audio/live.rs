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

use super::AudioSource;
use super::CaptureClock;
use super::CaptureEvent;
use super::leg::LegEncoder;
use super::system;

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
    /// Disconnects when the capture thread exits. Nothing is ever sent on
    /// it — the thread owning the sender is the signal.
    done: std::sync::mpsc::Receiver<()>,
}

/// How long a stop waits for the capture thread before abandoning it.
///
/// Sized for a *healthy* teardown, which is milliseconds against a 20 ms
/// poll, and deliberately not for the pathological case: a thread parked
/// inside `AudioOutputUnitStart` does not come back, so a longer wait buys
/// nothing and is paid by the Operator on every stop that hits it.
const STOP_GRACE: std::time::Duration = std::time::Duration::from_millis(2_000);

impl StreamHandle for ThreadStream {
    fn stop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let Some(handle) = self.handle.take() else {
            return;
        };
        // Never an unbounded join. `AudioOutputUnitStart` can block
        // indefinitely when the default input device changes underneath it
        // — plugging in AirPods mid-meeting is enough — and the thread is
        // then stuck before the loop that would see the stop flag. Joining
        // it there deadlocks stop, and with it the whole Core: even
        // `status` stops answering, and only SIGKILL recovers.
        match self.done.recv_timeout(STOP_GRACE) {
            // The sender went with the thread: it is finished, so this
            // join returns at once.
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let _ = handle.join();
            }
            _ => warn!(
                "the microphone thread did not stop within {STOP_GRACE:?}; \
                 abandoning it so the Meeting can still be finalized"
            ),
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
        // On Windows this must not go through cpal at all. Its host
        // initialisation *aborts* on a machine whose audio stack is not in a
        // state it expects — not a panic, so `catch_unwind` was tried and
        // could not help — and a question about whether a device exists has
        // no business being able to end the process. WASAPI answers it
        // directly, the same way the system-audio side does.
        #[cfg(target_os = "windows")]
        {
            use windows::Win32::Media::Audio::IMMDeviceEnumerator;
            use windows::Win32::Media::Audio::MMDeviceEnumerator;
            use windows::Win32::Media::Audio::eCapture;
            use windows::Win32::Media::Audio::eMultimedia;
            use windows::Win32::System::Com::CLSCTX_ALL;
            use windows::Win32::System::Com::COINIT_MULTITHREADED;
            use windows::Win32::System::Com::CoCreateInstance;
            use windows::Win32::System::Com::CoInitializeEx;

            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                let Ok(enumerator) = CoCreateInstance::<_, IMMDeviceEnumerator>(
                    &MMDeviceEnumerator,
                    None,
                    CLSCTX_ALL,
                ) else {
                    return false;
                };
                enumerator
                    .GetDefaultAudioEndpoint(eCapture, eMultimedia)
                    .is_ok()
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            cpal::default_host().default_input_device().is_some()
        }
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
            .context("no microphone is connected")?;
        let name = device.name().unwrap_or_else(|_| "unknown".to_string());
        // A device can exist and still be unusable, and the platform error
        // for it says nothing an Operator can act on — a machine with no
        // microphone at all still reports a default input device, then
        // refuses to describe it, and so does one where recording has not
        // been permitted. Name both causes rather than forwarding a message
        // about a Rust binding.
        let config = device.default_input_config().map_err(|error| {
            debug!(device = %name, %error, "the input device could not be configured");
            anyhow::anyhow!(
                "no usable microphone — check that one is connected, and that \
                 EverTranscript is allowed to use it in System Settings › \
                 Privacy & Security › Microphone"
            )
        })?;
        info!(device = %name, rate = config.sample_rate().0, channels = config.channels(), "microphone capture starting");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_stop = std::sync::Arc::clone(&stop);
        let (done_tx, done) = std::sync::mpsc::channel::<()>();
        let (ready_tx, ready) = std::sync::mpsc::channel::<std::result::Result<(), String>>();

        // cpal's Stream is !Send, so it is created and dropped on one
        // dedicated thread rather than moved into the async runtime.
        let handle = std::thread::Builder::new()
            .name("evertranscript-mic".to_string())
            .spawn(move || {
                // Held for the thread's lifetime; dropping it on the way
                // out is what tells `stop` the teardown finished.
                let _done = done_tx;
                if let Err(error) =
                    run_microphone(device, config, clock, events, thread_stop, ready_tx)
                {
                    warn!(%error, "microphone capture ended");
                }
            })
            .context("spawning the microphone thread")?;

        // Registered before the wait below, so a microphone that never starts
        // is still torn down by `stop()` rather than left running unseen.
        self.stream = Some(Box::new(ThreadStream {
            stop,
            handle: Some(handle),
            done,
        }));

        // **Wait for the HAL to actually start it.** This function used to
        // return the moment the thread was spawned, which made the order the
        // caller chose meaningless: the real `play()` happened later, racing
        // whatever else touched CoreAudio. And an idle system-audio process
        // tap makes that race a deadlock — the tap delivers no callbacks when
        // nothing is playing, and starting an input AudioUnit in that state
        // never returns (`.scratch/capture-deadlock`). A microphone that is
        // already delivering survives the tap being created; one that is
        // still starting does not, so the wait is the fix.
        match ready.recv_timeout(MICROPHONE_START_TIMEOUT) {
            Ok(Ok(())) => Ok(name),
            Ok(Err(error)) => anyhow::bail!("{error}"),
            Err(_) => anyhow::bail!(
                "the microphone did not start within {}s — the device accepted the \
                 request and never returned",
                MICROPHONE_START_TIMEOUT.as_secs()
            ),
        }
    }

    /// Whether system audio can be captured on this machine, and why not if
    /// it cannot. Asks the platform rather than guessing from the OS version,
    /// because the usual answer is an ungranted permission.
    pub fn system_audio_available() -> std::result::Result<(), String> {
        // Asking must never be able to take the Core down with it.
        //
        // The preflight calls this before a Meeting, and the audio stack
        // underneath is entitled to be in a state nobody anticipated: a
        // Windows runner with no audio device and the audio service stopped
        // made cpal's host initialisation abort the whole process — in the
        // test whose name is precisely this promise. A machine that cannot
        // answer is a machine whose answer is no.
        std::panic::catch_unwind(|| {
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
        })
        .unwrap_or_else(|_| {
            Err("this machine's audio system could not be asked about system audio".to_string())
        })
    }
}

impl AudioSource for LiveSource {
    fn start(&mut self, clock: CaptureClock, events: mpsc::Sender<CaptureEvent>) -> Result<()> {
        // Both legs are attempted, and neither can veto the other: a failure
        // below records that leg as unavailable and keeps going.
        //
        // **The order is load-bearing, and used to be documented as not
        // being.** An idle system-audio tap deadlocks an input AudioUnit that
        // is still starting, so the microphone goes first and
        // `start_microphone` does not return until CoreAudio has actually
        // started it. A microphone already delivering survives the tap.
        let microphone = match self.start_microphone(clock.clone(), events.clone()) {
            Ok(name) => {
                self.description = format!("live ({name})");
                None
            }
            Err(error) => {
                warn!(%error, "recording without a microphone");
                Some(format!("{error:#}"))
            }
        };

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

        // Named once both answers are in, so it describes what is actually
        // running rather than what had started so far.
        self.description = match (&microphone, &self.system) {
            (None, Some(capture)) => format!("{} + {}", self.description, capture.describe()),
            (None, None) => format!("{}, microphone only", self.description),
            (Some(_), Some(capture)) => format!("live ({}, system audio only)", capture.describe()),
            (Some(_), None) => "live (nothing available)".to_string(),
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
            // Deliberately not a let-chain: sending is the point of this
            // loop, and folding it into a condition would read as a test of
            // something rather than the announcement it is.
            let Some(reason) = reason else { continue };
            if events
                .try_send(CaptureEvent::Unavailable { channel, reason })
                .is_err()
            {
                debug!("nobody is listening for capture events yet");
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

/// How long CoreAudio may take to start the microphone before the leg is
/// called dead.
///
/// Starting an AudioUnit is milliseconds when it works and forever when it
/// does not, so this is short. Losing three seconds off the front of a
/// Meeting would be bad; waiting forever loses the Meeting.
const MICROPHONE_START_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

fn run_microphone(
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
    clock: CaptureClock,
    events: mpsc::Sender<CaptureEvent>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ready: std::sync::mpsc::Sender<std::result::Result<(), String>>,
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
    // `play()` is where this deadlocks when a system-audio tap is already
    // open and idle, so the signal goes *after* it returns rather than after
    // the stream is built. See `start_microphone` for why anyone waits.
    let started = stream.play().context("starting the microphone stream");
    let _ = ready.send(
        started
            .as_ref()
            .map(|_| ())
            .map_err(|error| format!("{error:#}")),
    );
    started?;

    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    drop(stream);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The deadlock this closes: a capture thread stuck inside
    /// `AudioOutputUnitStart` never reaches the loop that reads the stop
    /// flag, and an unbounded join on it wedged the entire Core — stop
    /// never returned, `status` stopped answering, and only SIGKILL got
    /// the machine back.
    #[test]
    fn a_capture_thread_that_never_notices_the_flag_does_not_hang_the_stop() {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (done_tx, done) = std::sync::mpsc::channel::<()>();
        let handle = std::thread::spawn(move || {
            let _done = done_tx;
            // Stuck the way CoreAudio gets stuck: never looks at the flag.
            std::thread::sleep(std::time::Duration::from_secs(120));
        });

        let mut stream = ThreadStream {
            stop: std::sync::Arc::clone(&stop),
            handle: Some(handle),
            done,
        };

        let started = std::time::Instant::now();
        stream.stop();
        let waited = started.elapsed();

        assert!(
            waited < STOP_GRACE * 2,
            "stop waited {waited:?} on a thread that never exits; it must give \
             up after about {STOP_GRACE:?} rather than block forever"
        );
        assert!(
            stop.load(std::sync::atomic::Ordering::Relaxed),
            "the thread must still have been asked to stop"
        );
    }

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
        let microphone = if LiveSource::microphone_available() {
            let (tx, _rx) = mpsc::channel(4);
            let mut probe = LiveSource::new();
            let started = probe.start_microphone(CaptureClock::start(), tx).is_ok();
            probe.stop();
            started
        } else {
            // Opening a device this machine has already said it does not
            // have is how a headless runner ends up aborting mid-suite.
            false
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
