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
//!
//! **The microphone comes back.** A device that vanishes mid-meeting —
//! AirPods running flat, a Bluetooth drop, a USB mic pulled — ends one
//! *attempt*, not the leg: the leg waits 1s, 2s, then 4s, and reopens the
//! default input, skipping the device that just failed while another exists.
//! Five seconds with no frames after the leg's first counts as a failure too.
//! The capture clock runs across the gap, so the joiner fills it with silence
//! and the file stays as long as the meeting. A fourth failure in a row ends
//! the leg, and the record says why.
//!
//! That reopen loop is PORTED from `fastrepl/anarlog`, Copyright (c)
//! 2023-present Fastrepl, Inc., licensed MIT: `run_channel` in
//! `crates/audio-actual/src/capture/recovery.rs` (the loop, the frame
//! timeout, the 1s/2s/4s ladder, the reset after 30s of health) and
//! `new_excluding` and `StartupPermit` in `crates/audio-actual/src/mic.rs`,
//! at rev `693500220d`. Upstream's loop is async over a stream it pulls;
//! this one is a thread watching cpal's pushed callbacks. See `PORTS.md`.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

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

    /// Starts the microphone leg, returning the first device's name.
    ///
    /// The leg is a thread running [`run_mic_leg`], which opens the device,
    /// watches it, and reopens it when it fails. Only the first open decides
    /// whether this succeeds: a microphone that cannot start at all is
    /// reported unavailable, as before, rather than retried.
    fn start_microphone(
        &mut self,
        clock: CaptureClock,
        events: mpsc::Sender<CaptureEvent>,
    ) -> Result<String> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let (done_tx, done) = std::sync::mpsc::channel::<()>();
        let (ready_tx, ready) = std::sync::mpsc::channel::<std::result::Result<String, String>>();

        let handle = std::thread::Builder::new()
            .name("evertranscript-mic-leg".to_string())
            .spawn(move || {
                // Held for the thread's lifetime; dropping it on the way
                // out is what tells `stop` the teardown finished.
                let _done = done_tx;
                let opener_events = events.clone();
                let open = |excluded: &[String], signals: Signals| {
                    open_microphone(excluded, signals, &clock, &opener_events)
                };
                run_mic_leg(open, LADDER, &events, &thread_stop, ready_tx);
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
        //
        // The opener enforces `MICROPHONE_START_TIMEOUT` itself; the margin
        // here only catches a leg thread that never answers at all.
        match ready.recv_timeout(MICROPHONE_START_TIMEOUT + Duration::from_secs(1)) {
            Ok(Ok(name)) => Ok(name),
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
    /// Starts the microphone and **only** the microphone.
    ///
    /// For capture that is about the Operator's own voice and nothing else —
    /// the enrolment. A recording made to answer "what do you sound like?"
    /// has no use for the far end, and opening a system-audio tap to throw
    /// its samples away would mean the product taps the machine's output
    /// during a step whose entire purpose is a person recording themselves.
    /// Refusing at the source is a cheaper promise to keep than one made in
    /// a comment above a filter.
    ///
    /// Unlike [`AudioSource::start`], a missing microphone is fatal: there is
    /// no second leg to degrade to.
    fn start_microphone_only(
        &mut self,
        clock: CaptureClock,
        events: mpsc::Sender<CaptureEvent>,
    ) -> Result<()> {
        let name = self.start_microphone(clock, events)?;
        self.description = format!("live ({name}, microphone only)");
        Ok(())
    }

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
const MICROPHONE_START_TIMEOUT: Duration = Duration::from_secs(3);

/// The microphone's reopen ladder: upstream's constants.
#[derive(Debug, Clone, Copy)]
struct Ladder {
    /// Waits before each reopen. A failure with no delay left ends the leg,
    /// which is the one [`super::supervisor::RestartBudget`] counts past.
    retry_delays: [Duration; 3],
    /// How long a leg that has delivered may go without a frame. A
    /// microphone in a silent room still delivers frames of zeros, so no
    /// frames at all is a dead device, not a quiet one.
    frame_timeout: Duration,
    /// How long an attempt must deliver before its earlier failures are
    /// forgiven.
    healthy_reset: Duration,
    /// How often the loop looks at the attempt it is watching.
    poll: Duration,
}

const LADDER: Ladder = Ladder {
    retry_delays: [
        Duration::from_secs(1),
        Duration::from_secs(2),
        Duration::from_secs(4),
    ],
    frame_timeout: Duration::from_secs(5),
    healthy_reset: Duration::from_secs(30),
    poll: Duration::from_millis(20),
};

/// What one open microphone's callbacks tell the loop watching it.
#[derive(Clone)]
struct Signals {
    /// Set by the data callback, cleared each time the loop looks.
    delivered: Arc<AtomicBool>,
    /// The error callback's report. The first one ends the attempt.
    failed: std::sync::mpsc::Sender<String>,
}

/// One open microphone. Stopping `stream` closes it.
struct Opened {
    device: String,
    stream: Box<dyn StreamHandle>,
}

#[derive(Debug)]
enum OpenError {
    /// The device refused, or there was none. Worth another try later.
    Failed(String),
    /// The start never returned. It still holds the startup permit, so
    /// nothing can be tried again on this leg.
    Hung(String),
}

/// One microphone start in flight at a time, process-wide.
///
/// `AudioOutputUnitStart` can block forever (`.scratch/capture-deadlock`), and
/// a native call cannot be cancelled. While one is stuck, a second start would
/// only pile another stuck thread on top of it; refusing it turns the next
/// attempt into an honest failure instead.
static MIC_STARTING: AtomicBool = AtomicBool::new(false);

/// How long a start waits for another one to finish before it is refused.
///
/// Upstream refuses at once and retries a second later. Here a Meeting's
/// *first* start is never retried, so an audio check starting in the same
/// instant would cost the Meeting its microphone. A healthy start takes
/// milliseconds; a stuck one never finishes. This stays under
/// `MICROPHONE_START_TIMEOUT`, so a refusal is reported as one.
const PERMIT_WAIT: Duration = Duration::from_secs(1);

/// Held across one start; dropping it lets the next one through.
struct StartupPermit(&'static AtomicBool);

impl StartupPermit {
    fn acquire(starting: &'static AtomicBool, wait: Duration) -> Result<Self> {
        let until = Instant::now() + wait;
        while starting
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            anyhow::ensure!(
                Instant::now() < until,
                "a previous microphone start is still pending"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(Self(starting))
    }
}

impl Drop for StartupPermit {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Runs the microphone leg until it is stopped or cannot come back.
///
/// Upstream's `run_channel`, over pushed callbacks rather than a pulled
/// stream. Every failed attempt is a [`CaptureEvent::StreamFailed`], so the
/// supervisor counts exactly the failures this loop does; a reopen onto
/// another device is a [`CaptureEvent::DeviceChanged`]; and the forgiveness
/// this loop grants is a [`CaptureEvent::Healthy`], so both counts reset
/// together. The first attempt only answers `ready`: a microphone that
/// cannot start at all is the caller's to report.
fn run_mic_leg(
    mut open: impl FnMut(&[String], Signals) -> std::result::Result<Opened, OpenError>,
    ladder: Ladder,
    events: &mpsc::Sender<CaptureEvent>,
    stop: &AtomicBool,
    ready: std::sync::mpsc::Sender<std::result::Result<String, String>>,
) {
    // The loop thread is not the audio thread, but a stop waits on it, so it
    // never blocks on a full queue either.
    let send = |event| {
        let _ = events.try_send(event);
    };
    let mut ready = Some(ready);
    let mut failures = 0;
    let mut excluded: Vec<String> = Vec::new();
    let mut device: Option<String> = None;
    let mut ever_delivered = false;
    loop {
        let (failed, failure) = std::sync::mpsc::channel();
        let signals = Signals {
            delivered: Arc::new(AtomicBool::new(false)),
            failed,
        };
        let error = match open(&excluded, signals.clone()) {
            Ok(mut opened) => {
                if let Some(ready) = ready.take() {
                    let _ = ready.send(Ok(opened.device.clone()));
                } else if device.as_deref() != Some(opened.device.as_str()) {
                    // The built-in mic hears the speakers; whatever was true
                    // of the last device is not true of this one.
                    send(CaptureEvent::DeviceChanged {
                        channel: AudioChannel::Mic,
                    });
                }
                device = Some(opened.device.clone());

                let opened_at = Instant::now();
                let mut last_frame = None;
                let mut healthy_since = None;
                let error = loop {
                    if stop.load(Ordering::Relaxed) {
                        break None;
                    }
                    if let Ok(error) = failure.try_recv() {
                        break Some(error);
                    }
                    let now = Instant::now();
                    if signals.delivered.swap(false, Ordering::Relaxed) {
                        last_frame = Some(now);
                        ever_delivered = true;
                        let since = *healthy_since.get_or_insert(now);
                        if failures > 0 && now.duration_since(since) >= ladder.healthy_reset {
                            failures = 0;
                            excluded.clear();
                            send(CaptureEvent::Healthy {
                                channel: AudioChannel::Mic,
                            });
                        }
                    } else if ever_delivered
                        && now.duration_since(last_frame.unwrap_or(opened_at))
                            >= ladder.frame_timeout
                    {
                        // Before the leg's first frame, a microphone that
                        // never delivers is the recorder's silent-leg
                        // watchdog's to judge, not this loop's.
                        break Some("the microphone stopped delivering audio".to_string());
                    }
                    std::thread::sleep(ladder.poll);
                };
                opened.stream.stop();
                let Some(error) = error else { return };
                excluded.push(opened.device);
                error
            }
            Err(OpenError::Failed(error)) | Err(OpenError::Hung(error)) if ready.is_some() => {
                if let Some(ready) = ready.take() {
                    let _ = ready.send(Err(error));
                }
                return;
            }
            Err(OpenError::Hung(reason)) => {
                warn!(reason, "the microphone did not come back; ending its leg");
                send(CaptureEvent::Unavailable {
                    channel: AudioChannel::Mic,
                    reason,
                });
                return;
            }
            Err(OpenError::Failed(error)) => error,
        };

        warn!(error, failures, "the microphone failed; reopening it");
        send(CaptureEvent::StreamFailed {
            channel: AudioChannel::Mic,
            error,
        });
        // No delay left: the supervisor ends the leg on this same failure.
        let Some(delay) = ladder.retry_delays.get(failures).copied() else {
            return;
        };
        failures += 1;
        let until = Instant::now() + delay;
        while Instant::now() < until {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(ladder.poll.min(delay));
        }
    }
}

/// Opens the microphone, skipping `excluded` devices while another exists,
/// and waits for CoreAudio to start it.
fn open_microphone(
    excluded: &[String],
    signals: Signals,
    clock: &CaptureClock,
    events: &mpsc::Sender<CaptureEvent>,
) -> std::result::Result<Opened, OpenError> {
    let (device, name) = pick_input(excluded).map_err(OpenError::Failed)?;
    // A device can exist and still be unusable, and the platform error
    // for it says nothing an Operator can act on — a machine with no
    // microphone at all still reports a default input device, then
    // refuses to describe it, and so does one where recording has not
    // been permitted. Name both causes rather than forwarding a message
    // about a Rust binding.
    let config = device.default_input_config().map_err(|error| {
        debug!(device = %name, %error, "the input device could not be configured");
        OpenError::Failed(
            "no usable microphone — check that one is connected, and that \
             EverTranscript is allowed to use it in System Settings › \
             Privacy & Security › Microphone"
                .to_string(),
        )
    })?;
    info!(device = %name, rate = config.sample_rate().0, channels = config.channels(), "microphone capture starting");

    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let (done_tx, done) = std::sync::mpsc::channel::<()>();
    let (ready_tx, ready) = std::sync::mpsc::channel::<std::result::Result<(), String>>();
    let clock = clock.clone();
    let events = events.clone();

    // cpal's Stream is !Send, so it is created and dropped on one
    // dedicated thread rather than moved into the async runtime.
    let handle = std::thread::Builder::new()
        .name("evertranscript-mic".to_string())
        .spawn(move || {
            let _done = done_tx;
            if let Err(error) = run_microphone(
                device,
                config,
                clock,
                events,
                signals,
                thread_stop,
                ready_tx,
            ) {
                warn!(%error, "microphone capture ended");
            }
        })
        .map_err(|error| OpenError::Failed(format!("spawning the microphone thread: {error}")))?;
    let mut stream = ThreadStream {
        stop,
        handle: Some(handle),
        done,
    };

    match ready.recv_timeout(MICROPHONE_START_TIMEOUT) {
        Ok(Ok(())) => Ok(Opened {
            device: name,
            stream: Box::new(stream),
        }),
        Ok(Err(error)) => {
            stream.stop();
            Err(OpenError::Failed(error))
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(OpenError::Failed(
            "the microphone thread ended before it started the device".to_string(),
        )),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            // Asked to stop, not waited for: it is parked inside the start,
            // and if that ever returns it must not go on recording unseen.
            stream.stop.store(true, Ordering::Relaxed);
            Err(OpenError::Hung(format!(
                "the microphone did not start within {}s — the device accepted the \
                 request and never returned",
                MICROPHONE_START_TIMEOUT.as_secs()
            )))
        }
    }
}

/// The default input, or upstream's `new_excluding`: when the default is a
/// device that just failed, any other input instead. macOS can still name a
/// vanished device as the default for a moment after it goes.
fn pick_input(excluded: &[String]) -> std::result::Result<(cpal::Device, String), String> {
    let host = cpal::default_host();
    let default = host
        .default_input_device()
        .ok_or_else(|| "no microphone is connected".to_string())?;
    let default_name = default.name().unwrap_or_else(|_| "unknown".to_string());
    if !excluded.contains(&default_name) {
        return Ok((default, default_name));
    }
    if let Ok(devices) = host.input_devices() {
        for device in devices {
            if let Ok(name) = device.name()
                && !excluded.contains(&name)
                && device.default_input_config().is_ok()
            {
                return Ok((device, name));
            }
        }
    }
    // Nothing else: the failed device is the only one there is.
    Ok((default, default_name))
}

fn run_microphone(
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
    clock: CaptureClock,
    events: mpsc::Sender<CaptureEvent>,
    signals: Signals,
    stop: Arc<AtomicBool>,
    ready: std::sync::mpsc::Sender<std::result::Result<(), String>>,
) -> Result<()> {
    // Every way this can fail before `play()` answers `ready` with why: a
    // `ready` that goes away unanswered is how the opener tells a start that
    // never returned from one that refused.
    let started = start_stream(&device, &config, clock, events, signals);
    let _ = ready.send(
        started
            .as_ref()
            .map(|_| ())
            .map_err(|error| format!("{error:#}")),
    );
    let stream = started?;

    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(stream);
    Ok(())
}

/// Builds the input stream and starts it, under the startup permit.
fn start_stream(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    clock: CaptureClock,
    events: mpsc::Sender<CaptureEvent>,
    signals: Signals,
) -> Result<cpal::Stream> {
    let mut encoder = LegEncoder::new(
        AudioChannel::Mic,
        config.channels() as usize,
        config.sample_rate().0,
        clock,
    )?;
    let delivered = signals.delivered;
    let failed = signals.failed;

    let data_callback = move |data: &[f32], _: &cpal::InputCallbackInfo| {
        let Some(frame) = encoder.encode(data) else {
            return;
        };
        delivered.store(true, Ordering::Relaxed);
        // Never block the audio thread: a full queue means the consumer is
        // behind, and dropping a frame is far better than glitching capture.
        // The joiner turns the dropped span into silence, so the timeline
        // stays honest.
        let _ = events.try_send(CaptureEvent::Frame(frame));
    };

    // The loop watching this attempt reports the failure, once; the
    // callback only says it happened.
    let error_callback = move |error: cpal::StreamError| {
        let _ = failed.send(error.to_string());
    };

    let _permit = StartupPermit::acquire(&MIC_STARTING, PERMIT_WAIT)?;
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
    stream.play().context("starting the microphone stream")?;
    Ok(stream)
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

    /// One scripted attempt of a fake microphone.
    enum Attempt {
        /// The open is refused.
        Refused,
        /// The open never returns, as the opener reports it.
        Hangs,
        /// The device delivers for a while, then does what `then` says.
        Delivers {
            device: &'static str,
            for_ms: u64,
            then: Then,
        },
    }

    enum Then {
        /// cpal's error callback fires, as on a vanished device.
        Errors,
        /// Frames simply stop.
        Stalls,
        /// It keeps delivering until stopped.
        Lasts,
    }

    /// Upstream's ladder, in milliseconds instead of seconds.
    const FAST: Ladder = Ladder {
        retry_delays: [
            Duration::from_millis(10),
            Duration::from_millis(20),
            Duration::from_millis(40),
        ],
        frame_timeout: Duration::from_millis(100),
        healthy_reset: Duration::from_millis(200),
        poll: Duration::from_millis(2),
    };

    struct Run {
        ready: std::result::Result<String, String>,
        events: Vec<CaptureEvent>,
        /// What each open was asked to skip, in order.
        excluded: Vec<Vec<String>>,
    }

    impl Run {
        fn kinds(&self) -> Vec<&'static str> {
            self.events
                .iter()
                .map(|event| match event {
                    CaptureEvent::StreamFailed { .. } => "failed",
                    CaptureEvent::DeviceChanged { .. } => "changed",
                    CaptureEvent::Healthy { .. } => "healthy",
                    CaptureEvent::Unavailable { .. } => "unavailable",
                    CaptureEvent::Degraded { .. } => "degraded",
                    CaptureEvent::Frame(_) => "frame",
                })
                .collect()
        }

        /// What the recorder's supervisor makes of the same events.
        fn verdicts(&self) -> Vec<super::super::supervisor::Action> {
            let mut policy = super::super::supervisor::ChurnPolicy::default();
            self.events
                .iter()
                .map(|event| policy.decide(event))
                .collect()
        }
    }

    /// Runs the leg against `script` until it ends by itself, or until
    /// `stop_after` when it would not.
    fn run_script(script: Vec<Attempt>, stop_after: Option<Duration>) -> Run {
        let script = std::sync::Mutex::new(std::collections::VecDeque::from(script));
        let excluded = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = Arc::clone(&excluded);
        let open = move |skip: &[String], signals: Signals| {
            seen.lock().unwrap().push(skip.to_vec());
            match script.lock().unwrap().pop_front() {
                None | Some(Attempt::Refused) => {
                    Err(OpenError::Failed("no microphone is connected".to_string()))
                }
                Some(Attempt::Hangs) => Err(OpenError::Hung("never returned".to_string())),
                Some(Attempt::Delivers {
                    device,
                    for_ms,
                    then,
                }) => {
                    let stop = Arc::new(AtomicBool::new(false));
                    let thread_stop = Arc::clone(&stop);
                    let (done_tx, done) = std::sync::mpsc::channel::<()>();
                    let handle = std::thread::spawn(move || {
                        let _done = done_tx;
                        let started = Instant::now();
                        let mut erred = false;
                        while !thread_stop.load(Ordering::Relaxed) {
                            let healthy = started.elapsed() < Duration::from_millis(for_ms);
                            match then {
                                _ if healthy => signals.delivered.store(true, Ordering::Relaxed),
                                Then::Lasts => signals.delivered.store(true, Ordering::Relaxed),
                                Then::Errors if !erred => {
                                    erred = true;
                                    let _ = signals.failed.send("device disappeared".to_string());
                                }
                                Then::Errors | Then::Stalls => {}
                            }
                            std::thread::sleep(Duration::from_millis(1));
                        }
                    });
                    Ok(Opened {
                        device: device.to_string(),
                        stream: Box::new(ThreadStream {
                            stop,
                            handle: Some(handle),
                            done,
                        }),
                    })
                }
            }
        };

        let (events_tx, mut events_rx) = mpsc::channel(1024);
        let stop = Arc::new(AtomicBool::new(false));
        let leg_stop = Arc::clone(&stop);
        let (ready_tx, ready) = std::sync::mpsc::channel();
        let leg = std::thread::spawn(move || {
            run_mic_leg(open, FAST, &events_tx, &leg_stop, ready_tx);
        });
        let ready = ready
            .recv_timeout(Duration::from_secs(5))
            .expect("the first attempt must answer");
        match stop_after {
            Some(after) => {
                std::thread::sleep(after);
                stop.store(true, Ordering::Relaxed);
            }
            None => {
                let deadline = Instant::now() + Duration::from_secs(5);
                while !leg.is_finished() {
                    assert!(
                        Instant::now() < deadline,
                        "a leg that cannot come back must end by itself"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
        leg.join().unwrap();

        let mut events = Vec::new();
        while let Ok(event) = events_rx.try_recv() {
            events.push(event);
        }
        let excluded = excluded.lock().unwrap().clone();
        Run {
            ready,
            events,
            excluded,
        }
    }

    /// The bug this closes: AirPods ran flat, cpal reported it once, and the
    /// Operator's own voice was silent for the rest of the Meeting while the
    /// record called it complete. Now the leg reopens, onto the built-in mic,
    /// and earns its budget back once it has delivered steadily.
    #[test]
    fn a_dropped_microphone_is_reopened_and_keeps_recording() {
        let run = run_script(
            vec![
                Attempt::Delivers {
                    device: "AirPods",
                    for_ms: 30,
                    then: Then::Errors,
                },
                Attempt::Refused,
                Attempt::Delivers {
                    device: "MacBook Pro Microphone",
                    for_ms: 0,
                    then: Then::Lasts,
                },
            ],
            Some(Duration::from_millis(600)),
        );

        assert_eq!(run.ready, Ok("AirPods".to_string()));
        assert_eq!(
            run.kinds(),
            vec!["failed", "failed", "changed", "healthy"],
            "two failures, a reopen onto another device, then health"
        );
        assert_eq!(
            run.excluded,
            vec![
                Vec::<String>::new(),
                vec!["AirPods".to_string()],
                vec!["AirPods".to_string()],
            ],
            "every reopen must skip the device that just failed"
        );
        assert!(
            !run.verdicts()
                .iter()
                .any(|action| matches!(action, super::super::supervisor::Action::EndLeg { .. })),
            "a microphone that came back must not be ended"
        );
    }

    #[test]
    fn a_microphone_that_stops_delivering_is_reopened() {
        let run = run_script(
            vec![
                Attempt::Delivers {
                    device: "USB Microphone",
                    for_ms: 30,
                    then: Then::Stalls,
                },
                Attempt::Delivers {
                    device: "USB Microphone",
                    for_ms: 0,
                    then: Then::Lasts,
                },
            ],
            // Past FAST's health threshold, as the dropped-mic test runs: stopping
            // just short of it left 40 ms that a slow CI runner overshot.
            Some(Duration::from_millis(600)),
        );

        assert_eq!(
            run.kinds(),
            vec!["failed", "healthy"],
            "a stall, then the reopened microphone delivering steadily"
        );
        match &run.events[0] {
            CaptureEvent::StreamFailed { error, .. } => assert!(
                error.contains("stopped delivering"),
                "a stall must say it is one, got {error}"
            ),
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    /// The other half of the contract: a microphone that cannot come back
    /// ends its leg, so the Meeting is marked partial with the reason.
    #[test]
    fn a_microphone_that_cannot_come_back_ends_the_leg() {
        let run = run_script(
            vec![Attempt::Delivers {
                device: "AirPods",
                for_ms: 30,
                then: Then::Errors,
            }],
            None,
        );

        assert_eq!(run.kinds(), vec!["failed"; 4]);
        match run.verdicts().last() {
            Some(super::super::supervisor::Action::EndLeg { channel, reason }) => {
                assert_eq!(*channel, AudioChannel::Mic);
                assert!(reason.contains("4 times in a row"), "got {reason}");
            }
            other => panic!("the leg's last failure must end it, got {other:?}"),
        }
    }

    #[test]
    fn a_reopen_that_never_returns_ends_the_leg_with_the_reason() {
        let run = run_script(
            vec![
                Attempt::Delivers {
                    device: "AirPods",
                    for_ms: 30,
                    then: Then::Errors,
                },
                Attempt::Hangs,
            ],
            None,
        );

        assert_eq!(run.kinds(), vec!["failed", "unavailable"]);
        assert!(matches!(
            run.verdicts().last(),
            Some(super::super::supervisor::Action::EndLeg { .. })
        ));
    }

    #[test]
    fn a_microphone_that_cannot_start_is_reported_rather_than_retried() {
        let run = run_script(vec![Attempt::Refused], None);
        assert!(run.ready.is_err());
        assert_eq!(run.excluded.len(), 1, "the first open is tried once");
        assert!(run.events.is_empty());
    }

    #[test]
    fn only_one_microphone_start_is_in_flight() {
        static STARTING: AtomicBool = AtomicBool::new(false);
        let first = StartupPermit::acquire(&STARTING, Duration::ZERO).expect("nothing is starting");
        let refused = StartupPermit::acquire(&STARTING, Duration::from_millis(20))
            .err()
            .expect("a second start must be refused while one is stuck");
        assert!(format!("{refused:#}").contains("still pending"));

        // A start that finishes promptly only makes the next one wait.
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            drop(first);
        });
        assert!(
            StartupPermit::acquire(&STARTING, Duration::from_secs(1)).is_ok(),
            "a start must wait out one that is merely in progress"
        );
        release.join().unwrap();
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
