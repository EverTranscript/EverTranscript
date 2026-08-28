//! System audio on macOS: a CoreAudio process tap.
//!
//! The shape of this, which is not guessable from the API names:
//!
//! 1. Create a **process tap** — a global mono mixdown of everything the
//!    machine is playing. A tap is an audio object, but it is not a device
//!    and nothing can read from it directly.
//! 2. Create a **private aggregate device** whose tap list contains that
//!    tap. Private means it does not appear in Audio MIDI Setup or in any
//!    other app's device list; we are not installing a virtual device on
//!    someone's machine to record a meeting.
//! 3. Install an IO proc on the aggregate device. Its *input* side is the
//!    tapped audio.
//!
//! The tap is unmuted, so the Operator still hears the meeting normally.
//!
//! All three resources outlive the process if leaked — a tap and an
//! aggregate device left behind by a crashed recorder are litter in the
//! machine's audio configuration — so teardown runs in reverse and is
//! driven by `Drop` as well as by `stop()`.
//!
//! **This requires macOS 14.4+ and the audio-capture permission.** Both
//! failures arrive as an `OSStatus` from the create calls, and both are
//! reported as an unavailable leg rather than as a broken recording.

use std::ffi::CStr;
use std::ffi::c_void;
use std::ptr::NonNull;

use anyhow::Context;
use anyhow::Result;
use evertranscript_protocol::AudioChannel;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_audio::AudioDeviceCreateIOProcID;
use objc2_core_audio::AudioDeviceDestroyIOProcID;
use objc2_core_audio::AudioDeviceIOProcID;
use objc2_core_audio::AudioDeviceStart;
use objc2_core_audio::AudioDeviceStop;
use objc2_core_audio::AudioHardwareCreateAggregateDevice;
use objc2_core_audio::AudioHardwareCreateProcessTap;
use objc2_core_audio::AudioHardwareDestroyAggregateDevice;
use objc2_core_audio::AudioHardwareDestroyProcessTap;
use objc2_core_audio::AudioObjectGetPropertyData;
use objc2_core_audio::AudioObjectGetPropertyDataSize;
use objc2_core_audio::AudioObjectID;
use objc2_core_audio::AudioObjectPropertyAddress;
use objc2_core_audio::CATapDescription;
use objc2_core_audio::CATapMuteBehavior;
use objc2_core_audio::kAudioAggregateDeviceIsPrivateKey;
use objc2_core_audio::kAudioAggregateDeviceIsStackedKey;
use objc2_core_audio::kAudioAggregateDeviceNameKey;
use objc2_core_audio::kAudioAggregateDeviceTapAutoStartKey;
use objc2_core_audio::kAudioAggregateDeviceTapListKey;
use objc2_core_audio::kAudioAggregateDeviceUIDKey;
use objc2_core_audio::kAudioHardwarePropertyDefaultOutputDevice;
use objc2_core_audio::kAudioHardwarePropertyProcessObjectList;
use objc2_core_audio::kAudioObjectPropertyElementMain;
use objc2_core_audio::kAudioObjectPropertyScopeGlobal;
use objc2_core_audio::kAudioObjectSystemObject;
use objc2_core_audio::kAudioProcessPropertyIsRunningOutput;
use objc2_core_audio::kAudioSubTapDriftCompensationKey;
use objc2_core_audio::kAudioSubTapUIDKey;
use objc2_core_audio::kAudioTapPropertyFormat;
use objc2_core_audio::kAudioTapPropertyUID;
use objc2_core_audio_types::AudioBufferList;
use objc2_core_audio_types::AudioStreamBasicDescription;
use objc2_core_audio_types::AudioTimeStamp;
use objc2_core_foundation::CFDictionary;
use objc2_foundation::NSArray;
use objc2_foundation::NSDictionary;
use objc2_foundation::NSNumber;
use objc2_foundation::NSString;
use objc2_foundation::NSUUID;
use tokio::sync::mpsc;
use tracing::debug;
use tracing::info;
use tracing::warn;

use super::SystemCapture;
use crate::audio::CaptureClock;
use crate::audio::CaptureEvent;
use crate::audio::leg::LegEncoder;

/// Set when a format is planar (`kAudioFormatFlagIsNonInterleaved`).
const NON_INTERLEAVED: u32 = 1 << 5;

/// Set when samples are floating point (`kAudioFormatFlagIsFloat`).
const IS_FLOAT: u32 = 1 << 0;

/// What the IO proc needs, reachable from a real-time audio thread.
///
/// Owned by `TapCapture` as a raw pointer because CoreAudio hands it back as
/// `void*`. Only the audio thread touches it while the device is running,
/// and `AudioDeviceStop` returns only once that thread is done, so the
/// exclusive access here is real rather than assumed.
struct TapContext {
    encoder: LegEncoder,
    events: mpsc::Sender<CaptureEvent>,
    /// Planar formats arrive one channel per buffer and are averaged here
    /// before encoding; interleaved formats leave this empty.
    downmix: Vec<f32>,
    planar_channels: usize,
    /// Milliseconds delivered before the first non-zero sample, and whether
    /// one has ever arrived. Together these detect a refused permission —
    /// see [`note_level`].
    silent_ms: u64,
    heard_audio: bool,
    reported_silence: bool,
    /// Whether anything was playing when last asked, and how long ago that
    /// was. Cached because the IO callback runs every few milliseconds and
    /// enumerating audio processes is far too costly to do there.
    output_running: bool,
    since_output_check_ms: u64,
}

impl TapContext {
    /// Whether anything is playing, re-asked about once a second.
    fn output_is_running(&mut self, duration_ms: u64) -> bool {
        self.since_output_check_ms += duration_ms;
        if self.since_output_check_ms >= OUTPUT_CHECK_INTERVAL_MS {
            self.since_output_check_ms = 0;
            self.output_running = anything_is_playing();
        }
        self.output_running
    }
}

/// How often to ask the system whether anything is playing.
const OUTPUT_CHECK_INTERVAL_MS: u64 = 1_000;

/// How much *played-but-silent* audio proves the permission was refused.
///
/// Measured against playback, not wall clock. The original form of this
/// check counted any silence at all, on the premise that "a global tap's
/// callback fires only while something is playing, so a machine with
/// nothing to record delivers no frames at all". That premise is false on
/// macOS 26: the tap delivers zero-filled frames continuously with nothing
/// playing, so a quiet meeting was accused of a permission it already had
/// (DECISIONS Q9, superseding Q3). The asymmetry the check needs is real,
/// but it has to be asked for rather than inferred — see
/// [`anything_is_playing`].
const SILENCE_PROVES_REFUSAL_MS: u64 = 15_000;

/// Watches for the one failure a created tap cannot rule out.
///
/// `AudioHardwareCreateProcessTap` succeeds whether or not the Operator has
/// granted audio recording; when they have not, it simply delivers digital
/// silence forever. Nothing in the API distinguishes the two, so a recorder
/// that trusts the return code produces meetings with the far end missing
/// and no explanation of why. This is the check that turns that into a
/// stated reason.
///
/// Returns the reason once, when it becomes certain.
///
/// `output_is_running` is the caller's answer to "is anything playing right
/// now": silence while nothing plays is the ordinary state of a quiet
/// meeting and proves nothing at all.
fn note_level(
    context: &mut TapContext,
    samples: &[f32],
    duration_ms: u64,
    output_is_running: bool,
) -> Option<String> {
    if context.heard_audio || context.reported_silence {
        return None;
    }
    if samples.iter().any(|sample| *sample != 0.0) {
        context.heard_audio = true;
        return None;
    }
    if !output_is_running {
        return None;
    }
    context.silent_ms += duration_ms;
    if context.silent_ms < SILENCE_PROVES_REFUSAL_MS {
        return None;
    }
    context.reported_silence = true;
    Some(
        "system audio is being played but arrives as silence — grant EverTranscript \
         permission to record system audio in System Settings › Privacy & Security, \
         then start the meeting again"
            .to_string(),
    )
}

/// A live tap: the tap object, the aggregate device reading it, and the IO
/// proc doing the reading.
pub struct TapCapture {
    tap: AudioObjectID,
    aggregate: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    context: *mut TapContext,
    running: bool,
    rate: f64,
}

// The raw pointer is the only reason this is not automatically Send. It is
// created here, handed to CoreAudio, and freed here after the device is
// stopped; no other thread ever holds it.
unsafe impl Send for TapCapture {}

impl SystemCapture for TapCapture {
    fn stop(&mut self) {
        self.teardown();
    }

    fn describe(&self) -> String {
        format!("CoreAudio process tap at {:.0} Hz", self.rate)
    }
}

impl Drop for TapCapture {
    fn drop(&mut self) {
        self.teardown();
    }
}

impl TapCapture {
    /// Releases everything, in the reverse of the order it was acquired.
    ///
    /// Safe to call twice: each step clears the handle it consumed, which
    /// matters because both `stop()` and `Drop` call this.
    fn teardown(&mut self) {
        unsafe {
            if self.running {
                status(
                    "AudioDeviceStop",
                    AudioDeviceStop(self.aggregate, self.proc_id),
                );
                self.running = false;
            }
            if let Some(proc_id) = self.proc_id.take() {
                status(
                    "AudioDeviceDestroyIOProcID",
                    AudioDeviceDestroyIOProcID(self.aggregate, Some(proc_id)),
                );
            }
            if self.aggregate != 0 {
                status(
                    "AudioHardwareDestroyAggregateDevice",
                    AudioHardwareDestroyAggregateDevice(self.aggregate),
                );
                self.aggregate = 0;
            }
            if self.tap != 0 {
                status(
                    "AudioHardwareDestroyProcessTap",
                    AudioHardwareDestroyProcessTap(self.tap),
                );
                self.tap = 0;
            }
            // Only now is the audio thread guaranteed to be done with it.
            if !self.context.is_null() {
                drop(Box::from_raw(self.context));
                self.context = std::ptr::null_mut();
            }
        }
    }
}

/// Logs a non-zero OSStatus during teardown without failing.
///
/// A device already going away reports errors that mean nothing to the
/// Operator; the recording is over by this point either way.
fn status(what: &str, code: i32) {
    if code != 0 {
        debug!(status = code, "{what} returned a non-zero status");
    }
}

pub fn start(
    clock: CaptureClock,
    events: mpsc::Sender<CaptureEvent>,
) -> Result<Box<dyn SystemCapture>> {
    // A machine with nothing to play through has nothing to tap. Checking
    // first turns "the recording is silent" into a reason.
    require_an_output_device()?;

    let tap = create_tap()?;
    // From here on every early return must release what we hold, so the tap
    // is wrapped in the guard immediately.
    let mut capture = TapCapture {
        tap,
        aggregate: 0,
        proc_id: None,
        context: std::ptr::null_mut(),
        running: false,
        rate: 0.0,
    };

    let format = tap_format(tap).context("reading the tap's audio format")?;
    anyhow::ensure!(
        format.mSampleRate > 0.0 && format.mChannelsPerFrame > 0,
        "the tap reported an unusable format ({} Hz, {} channels)",
        format.mSampleRate,
        format.mChannelsPerFrame
    );
    // Reading 16-bit samples as floats yields noise that still *sounds* like
    // a recording failure rather than a bug, so refuse instead of guessing.
    // Every current macOS output device taps as 32-bit float.
    anyhow::ensure!(
        format.mFormatFlags & IS_FLOAT != 0 && format.mBitsPerChannel == 32,
        "the tap produced {}-bit {} samples, and only 32-bit float is handled",
        format.mBitsPerChannel,
        if format.mFormatFlags & IS_FLOAT != 0 {
            "float"
        } else {
            "integer"
        }
    );
    capture.rate = format.mSampleRate;

    capture.aggregate = create_aggregate(tap)?;

    let planar = format.mFormatFlags & NON_INTERLEAVED != 0;
    let channels = format.mChannelsPerFrame as usize;
    let encoder = LegEncoder::new(
        AudioChannel::System,
        if planar { 1 } else { channels },
        format.mSampleRate as u32,
        clock,
    )?;
    let context = Box::into_raw(Box::new(TapContext {
        encoder,
        events,
        downmix: Vec::new(),
        planar_channels: if planar { channels } else { 0 },
        silent_ms: 0,
        heard_audio: false,
        reported_silence: false,
        output_running: false,
        since_output_check_ms: OUTPUT_CHECK_INTERVAL_MS,
    }));
    capture.context = context;

    let mut proc_id: AudioDeviceIOProcID = None;
    let created = unsafe {
        AudioDeviceCreateIOProcID(
            capture.aggregate,
            Some(io_proc),
            context as *mut c_void,
            NonNull::from(&mut proc_id),
        )
    };
    anyhow::ensure!(
        created == 0 && proc_id.is_some(),
        "installing the audio callback failed (OSStatus {created})"
    );
    capture.proc_id = proc_id;

    let started = unsafe { AudioDeviceStart(capture.aggregate, capture.proc_id) };
    anyhow::ensure!(
        started == 0,
        "starting system-audio capture failed (OSStatus {started})"
    );
    capture.running = true;

    info!(
        rate = format.mSampleRate,
        channels, planar, "system-audio capture started via a process tap"
    );
    Ok(Box::new(capture))
}

/// Builds the tap: a mono mixdown of everything playing.
///
/// Mono because a leg is one voice-bearing channel, and the stereo image of
/// a conference call carries nothing a transcript needs.
fn create_tap() -> Result<AudioObjectID> {
    // Exclude nothing: the Core plays no audio, so a global tap cannot hear
    // itself. If playback is ever added, its process object belongs here —
    // otherwise the recording would capture its own output.
    let exclude: Retained<NSArray<NSNumber>> = NSArray::new();
    let description = unsafe {
        let description = CATapDescription::initMonoGlobalTapButExcludeProcesses(
            CATapDescription::alloc(),
            &exclude,
        );
        description.setName(&NSString::from_str("EverTranscript system audio"));
        description.setUUID(&NSUUID::new());
        // Private: this tap is ours, and no other app should see it.
        description.setPrivate(true);
        // Unmuted, so the Operator still hears the meeting. This is the
        // default, but a recorder that silences the call it is recording is
        // a bad enough failure to be worth stating.
        description.setMuteBehavior(CATapMuteBehavior(0));
        description
    };

    let mut tap: AudioObjectID = 0;
    let created = unsafe { AudioHardwareCreateProcessTap(Some(&description), &mut tap) };
    if created != 0 || tap == 0 {
        anyhow::bail!("{}", tap_failure(created));
    }
    Ok(tap)
}

/// Turns the OSStatus from tap creation into something an Operator can act on.
fn tap_failure(code: i32) -> String {
    // 1852797029 is 'nope' — what CoreAudio returns when the caller is not
    // permitted to tap audio. It is by far the most likely failure, and
    // "OSStatus 1852797029" tells nobody what to do about it.
    const NOT_PERMITTED: i32 = 1_852_797_029;
    match code {
        NOT_PERMITTED => "permission to record system audio has not been granted \
             — grant EverTranscript audio recording in System Settings › Privacy & Security"
            .to_string(),
        other => format!(
            "creating the system-audio tap failed (OSStatus {other}) \
             — system audio needs macOS 14.4 or later"
        ),
    }
}

/// Creates the private aggregate device that makes the tap readable.
///
/// **The tap is the only member.** Adding the output device to the
/// sub-device list as well — which reads like the natural thing to do, and
/// which every example that predates taps does — records the same audio
/// twice: once through the device and once through the tap *of* that device.
/// The result is an audible echo on every recording, and it is subtle enough
/// that Meetily shipped it and had to fix it in place. `master` without a
/// sub-device list is a no-op, so that key is omitted too.
fn create_aggregate(tap: AudioObjectID) -> Result<AudioObjectID> {
    let tap_uid = tap_uid(tap).context("reading the tap's UID")?;
    let yes = NSNumber::new_bool(true);
    let no = NSNumber::new_bool(false);

    let sub_tap = NSDictionary::from_slices(
        &[
            &*key(kAudioSubTapUIDKey),
            &*key(kAudioSubTapDriftCompensationKey),
        ],
        &[&*tap_uid as &AnyObject, &*yes as &AnyObject],
    );

    let uid = NSUUID::new().UUIDString();
    let keys: Vec<Retained<NSString>> = [
        kAudioAggregateDeviceNameKey,
        kAudioAggregateDeviceUIDKey,
        kAudioAggregateDeviceIsPrivateKey,
        kAudioAggregateDeviceIsStackedKey,
        kAudioAggregateDeviceTapAutoStartKey,
        kAudioAggregateDeviceTapListKey,
    ]
    .into_iter()
    .map(key)
    .collect();
    let name = NSString::from_str("EverTranscript");
    let sub_taps = NSArray::from_slice(&[&*sub_tap]);
    let values: [&AnyObject; 6] = [
        &name, &uid, &yes, // private: invisible to every other app
        &no,  // not stacked
        &yes, // start the tap with the device
        &sub_taps,
    ];
    let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let description: Retained<NSDictionary<NSString, AnyObject>> =
        NSDictionary::from_slices(&key_refs, &values);

    // NSDictionary and CFDictionary are the same object; the cast is the
    // toll-free bridge, not a reinterpretation.
    let as_cf = unsafe { &*(Retained::as_ptr(&description) as *const CFDictionary) };
    let mut aggregate: AudioObjectID = 0;
    let created =
        unsafe { AudioHardwareCreateAggregateDevice(as_cf, NonNull::from(&mut aggregate)) };
    anyhow::ensure!(
        created == 0 && aggregate != 0,
        "creating the private aggregate device failed (OSStatus {created})"
    );
    Ok(aggregate)
}

/// An aggregate-device dictionary key as an `NSString`.
fn key(name: &CStr) -> Retained<NSString> {
    NSString::from_str(name.to_str().unwrap_or_default())
}

fn address(selector: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    }
}

/// Whether any process on this machine is currently playing audio.
///
/// This is the question the refusal check depends on, and asking it outright
/// is the whole of the fix in DECISIONS Q9. Measured on macOS 26 before it
/// was relied on: false with nothing playing, true while a process plays,
/// and — the case that decides whether it is usable at all — still false
/// while our own tap is capturing, so the recorder does not see itself and
/// no self-exclusion is needed here.
pub(crate) fn anything_is_playing() -> bool {
    let mut addr = address(kAudioHardwarePropertyProcessObjectList);
    let mut size: u32 = 0;
    let code = unsafe {
        AudioObjectGetPropertyDataSize(
            kAudioObjectSystemObject as AudioObjectID,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
        )
    };
    if code != 0 || size == 0 {
        return false;
    }

    let mut processes =
        vec![0 as AudioObjectID; size as usize / std::mem::size_of::<AudioObjectID>()];
    let code = unsafe {
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject as AudioObjectID,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(processes.as_mut_slice()).cast(),
        )
    };
    if code != 0 {
        return false;
    }
    processes.iter().copied().any(is_running_output)
}

/// Whether one audio process object is playing. A property that cannot be
/// read is treated as "not playing": this gates an accusation, so the
/// failure that costs nothing is the one that stays quiet.
fn is_running_output(process: AudioObjectID) -> bool {
    let mut addr = address(kAudioProcessPropertyIsRunningOutput);
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let code = unsafe {
        AudioObjectGetPropertyData(
            process,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        )
    };
    code == 0 && value == 1
}

fn require_an_output_device() -> Result<()> {
    let mut device: AudioObjectID = 0;
    let mut size = std::mem::size_of::<AudioObjectID>() as u32;
    let mut addr = address(kAudioHardwarePropertyDefaultOutputDevice);
    let code = unsafe {
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject as AudioObjectID,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut device).cast(),
        )
    };
    anyhow::ensure!(code == 0, "no default output device (OSStatus {code})");
    anyhow::ensure!(
        device != 0,
        "this machine has no audio output, so there is nothing to record"
    );
    Ok(())
}

/// The tap's UID, which is how the aggregate device refers to it.
///
/// CoreAudio hands back a retained string; `from_raw` takes that reference
/// rather than adding one, which is what balances it.
fn tap_uid(tap: AudioObjectID) -> Result<Retained<NSString>> {
    property_string(tap, kAudioTapPropertyUID)
}

fn property_string(object: AudioObjectID, selector: u32) -> Result<Retained<NSString>> {
    let mut value: *const NSString = std::ptr::null();
    let mut size = std::mem::size_of::<*const NSString>() as u32;
    let mut addr = address(selector);
    let code = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        )
    };
    anyhow::ensure!(
        code == 0,
        "reading a CoreAudio string failed (OSStatus {code})"
    );
    unsafe { Retained::from_raw(value as *mut NSString) }
        .context("CoreAudio returned no string for a property it reported as readable")
}

fn tap_format(tap: AudioObjectID) -> Result<AudioStreamBasicDescription> {
    // No Default impl; a zeroed ASBD is what CoreAudio expects to fill in.
    let mut format: AudioStreamBasicDescription = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<AudioStreamBasicDescription>() as u32;
    let mut addr = address(kAudioTapPropertyFormat);
    let code = unsafe {
        AudioObjectGetPropertyData(
            tap,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut format).cast(),
        )
    };
    anyhow::ensure!(
        code == 0,
        "the tap has no readable format (OSStatus {code})"
    );
    Ok(format)
}

/// Called by CoreAudio on a real-time thread for every capture cycle.
///
/// Everything here must be non-blocking. `try_send` drops the frame when the
/// consumer is behind, which the joiner turns into silence on the timeline —
/// a visible gap, rather than audio that quietly shifts everything after it.
unsafe extern "C-unwind" fn io_proc(
    _device: AudioObjectID,
    _now: NonNull<AudioTimeStamp>,
    input_data: NonNull<AudioBufferList>,
    _input_time: NonNull<AudioTimeStamp>,
    _output_data: NonNull<AudioBufferList>,
    _output_time: NonNull<AudioTimeStamp>,
    client_data: *mut c_void,
) -> OSStatusCompat {
    if client_data.is_null() {
        return 0;
    }
    let context = unsafe { &mut *(client_data as *mut TapContext) };
    let list = unsafe { input_data.as_ref() };
    let count = list.mNumberBuffers as usize;
    if count == 0 {
        return 0;
    }

    // mBuffers is declared as a one-element array and indexed past its end;
    // that is the C idiom AudioBufferList is built on.
    let buffers = unsafe { std::slice::from_raw_parts(list.mBuffers.as_ptr(), count) };

    let frame = if context.planar_channels > 1 && count > 1 {
        // Planar: one channel per buffer, averaged into mono.
        let per_channel = buffers[0].mDataByteSize as usize / std::mem::size_of::<f32>();
        context.downmix.clear();
        context.downmix.resize(per_channel, 0.0);
        let mut used = 0usize;
        for buffer in buffers {
            if buffer.mData.is_null() {
                continue;
            }
            let samples = unsafe {
                std::slice::from_raw_parts(
                    buffer.mData as *const f32,
                    (buffer.mDataByteSize as usize / std::mem::size_of::<f32>()).min(per_channel),
                )
            };
            for (sum, sample) in context.downmix.iter_mut().zip(samples) {
                *sum += *sample;
            }
            used += 1;
        }
        if used == 0 {
            return 0;
        }
        let scale = 1.0 / used as f32;
        for sample in context.downmix.iter_mut() {
            *sample *= scale;
        }
        let downmix = std::mem::take(&mut context.downmix);
        let frame = context.encoder.encode(&downmix);
        context.downmix = downmix;
        frame
    } else {
        let buffer = &buffers[0];
        if buffer.mData.is_null() {
            return 0;
        }
        let samples = unsafe {
            std::slice::from_raw_parts(
                buffer.mData as *const f32,
                buffer.mDataByteSize as usize / std::mem::size_of::<f32>(),
            )
        };
        context.encoder.encode(samples)
    };

    if let Some(frame) = frame {
        let duration_ms = frame.duration_ms();
        // Only while there is still something to decide. One real sample,
        // or one reported reason, settles this permanently — and asking
        // anyway would enumerate every audio process on the machine once a
        // second for the rest of the meeting, which is the cost this
        // function's own comment warns about.
        let settled = context.heard_audio || context.reported_silence;
        let playing = !settled && context.output_is_running(duration_ms);
        if let Some(reason) = note_level(context, &frame.samples, duration_ms, playing) {
            // Said once, and the leg stays attached. Ending it here would
            // make a wrong accusation unrecoverable for the rest of the
            // meeting, and the frames still have to flow either way or the
            // joiner waits forever on a leg that is talking to it.
            let _ = context.events.try_send(CaptureEvent::Degraded {
                channel: AudioChannel::System,
                reason,
            });
        }
        let _ = context.events.try_send(CaptureEvent::Frame(frame));
    }
    0
}

/// `OSStatus` under whatever name the bindings give it.
type OSStatusCompat = i32;

/// Reports whether a tap can be built on this machine, without recording.
///
/// **`Ok` is not a promise that audio will arrive.** macOS grants the tap
/// and then delivers silence when the audio-recording permission has been
/// refused, and offers no way to tell the two apart until something plays.
/// What this rules out is the rest: too old an OS, no output device, a
/// format we cannot read. The refused-permission case is caught during
/// capture by [`note_level`] instead, which is the only place the evidence
/// exists.
pub fn available() -> std::result::Result<(), String> {
    match create_tap() {
        Ok(tap) => {
            unsafe { AudioHardwareDestroyProcessTap(tap) };
            Ok(())
        }
        Err(error) => {
            warn!(%error, "system-audio capture is not available");
            Err(error.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These tests are all about a tap that something is playing into:
    /// that is the only situation in which silence accuses anyone.
    const PLAYING: bool = true;

    fn context() -> TapContext {
        let (events, _rx) = mpsc::channel(8);
        TapContext {
            encoder: LegEncoder::new(AudioChannel::System, 1, 48_000, CaptureClock::start())
                .expect("encoder"),
            events,
            downmix: Vec::new(),
            planar_channels: 0,
            silent_ms: 0,
            heard_audio: false,
            reported_silence: false,
            output_running: false,
            since_output_check_ms: OUTPUT_CHECK_INTERVAL_MS,
        }
    }

    #[test]
    fn audio_that_arrives_as_silence_is_reported_as_a_refused_permission() {
        // The failure this exists for: the tap is created, frames flow, and
        // every one is zero because the Operator never granted recording.
        let mut context = context();
        let silence = vec![0.0f32; 4_800]; // 100 ms
        let mut reason = None;
        for _ in 0..(SILENCE_PROVES_REFUSAL_MS / 100) {
            reason = reason.or(note_level(&mut context, &silence, 100, PLAYING));
        }
        let reason = reason.expect("silence for the whole window must be reported");
        assert!(
            reason.contains("Privacy & Security"),
            "the reason must say how to fix it, got {reason}"
        );
    }

    #[test]
    fn silence_while_nothing_plays_never_accuses_anyone() {
        // The dogfood failure, in miniature. A tap delivers zero-filled
        // frames continuously on macOS 26 whether or not anything is
        // playing, so counting silence alone marked a correct recording
        // incomplete and told the Operator to grant a permission they
        // already had (DECISIONS Q9).
        let mut context = context();
        let silence = vec![0.0f32; 4_800]; // 100 ms

        // Ten times over the threshold, and still not evidence of anything.
        for _ in 0..(SILENCE_PROVES_REFUSAL_MS / 100 * 10) {
            assert!(
                note_level(&mut context, &silence, 100, false).is_none(),
                "a quiet meeting is not a refused permission"
            );
        }
        assert_eq!(
            context.silent_ms, 0,
            "silence with nothing playing must not even be counted"
        );

        // The same tap, once something is actually played into it, is still
        // caught: gating the evidence must not discard it.
        let mut reason = None;
        for _ in 0..(SILENCE_PROVES_REFUSAL_MS / 100) {
            reason = reason.or(note_level(&mut context, &silence, 100, PLAYING));
        }
        assert!(
            reason.is_some_and(|reason| reason.contains("Privacy & Security")),
            "played-but-silent audio is still a refused permission"
        );
    }

    #[test]
    fn it_is_said_once_rather_than_every_buffer() {
        let mut context = context();
        let silence = vec![0.0f32; 4_800];
        let mut reported = 0;
        for _ in 0..400 {
            if note_level(&mut context, &silence, 100, PLAYING).is_some() {
                reported += 1;
            }
        }
        assert_eq!(reported, 1, "a refused permission is stated once");
    }

    #[test]
    fn hearing_anything_at_all_settles_the_question_for_good() {
        // Permission cannot be revoked mid-meeting into silent-but-flowing
        // audio, so one real sample ends the check. Without this, a quiet
        // stretch late in a long meeting would accuse a working tap.
        let mut context = context();
        assert!(note_level(&mut context, &[0.0, 0.4, 0.0], 100, PLAYING).is_none());
        assert!(context.heard_audio);

        let silence = vec![0.0f32; 4_800];
        for _ in 0..400 {
            assert!(
                note_level(&mut context, &silence, 100, PLAYING).is_none(),
                "a tap that has produced audio is never accused of being silent"
            );
        }
    }

    #[test]
    fn a_short_silent_passage_is_not_enough_to_accuse_anyone() {
        let mut context = context();
        let silence = vec![0.0f32; 4_800];
        for _ in 0..10 {
            assert!(note_level(&mut context, &silence, 100, PLAYING).is_none());
        }
        assert_eq!(context.silent_ms, 1_000);
    }
}
