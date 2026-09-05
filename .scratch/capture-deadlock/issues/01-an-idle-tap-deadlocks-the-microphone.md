# An idle system-audio tap deadlocks the microphone, and the Meeting records nothing

Status: resolved

> **Corrected 2026-09-05, after the first write-up said the tap deadlocks the
> microphone outright. It does not.** A CoreAudio process tap delivers no
> callbacks at all when no process is playing audio — not silence, nothing —
> and *that* is the state in which starting cpal's input AudioUnit deadlocks.
> With audio playing, both legs work: 7490 ms on the microphone and 7560 ms on
> the system leg, in the same binary that hangs on a silent machine. Measured
> three ways: the tap alone reports 0 frames silent and 468 frames with a tone
> playing; `audio-check` hangs silent and succeeds with the tone; and cpal
> alone works either way. The history fits — 2026-09-01's working run was
> during a Teams call.
>
> So the trigger is **a Meeting that begins while nothing is playing**, which
> is an ordinary way for one to begin.

## Answer — fixed 2026-09-05 (`1222d04`)

The microphone starts before the tap, and `start_microphone` no longer returns
until CoreAudio has actually started it. Both halves were needed: reordering
alone changed nothing, because the function used to return the moment its
thread was spawned, so the real `play()` happened later and raced anyway. It
now signals after `play()` returns, with a three-second timeout after which the
leg is reported unavailable with a reason rather than hanging the Meeting.

A microphone already delivering survives the tap being created — measured at
+145,408 samples across its creation, against a hang when the two race.

Verified on the machine that produced this: silent, capture went from nothing
to 3950–5860 ms on the microphone; with a tone playing, both legs report in
full. The watchdog was adjusted in the same commit, because the fix made "the
system leg delivered nothing" the ordinary state of a Meeting that opens
quietly — the legs are now judged on their own terms.

Found 2026-09-05 on the author's M-series Mac, macOS 26.6.2, after a working
install on 2026-09-01. It is why every Meeting since has come out empty.

## What happens

`audio-check` reports `nothing captured` on **both** legs — not silence, zero
frames — with no reason attached and no `couldNotStart`, because both legs
report starting successfully:

```
INFO system-audio capture started via a process tap  rate=48000 channels=1
INFO system audio joined the recording  via=CoreAudio process tap at 48000 Hz
INFO microphone capture starting  device=MacBook Pro Microphone rate=48000 channels=1
WARN the microphone thread did not stop within 2s; abandoning it
```

The microphone thread is deadlocked inside CoreAudio:

```
AudioOutputUnitStart → AudioDeviceStart_mac_imp → HAL_HardwarePlugIn_DeviceStart
 → HALC_ProxyIOContext::StartIOProc → HALC_ProxyIOContext::_StartIO
  → HALB_IOThread::StartAndWaitForState → HALB_Guard::WaitFor
   → _pthread_cond_wait → _pthread_mutex_firstfit_lock_slow → __psynch_mutexwait
```

## It is ours, not the machine's — but only while the tap is idle

Isolated with a probe in our own binary, same device (`MacBook Pro Microphone`,
F32/48000/1ch), one variable:

| | Result |
| --- | --- |
| cpal alone | `play()` returns; **143,872 samples in 3s** (48000×3, exact) |
| `audio::system::start()` first, then cpal | `build_input_stream` never returns |

Independent evidence the machine is fine: `ffmpeg -f avfoundation -i ":0"`
records 85,248 samples with a non-zero peak, and `coreaudiod` is healthy.

Reordering `LiveSource::start` to build the microphone first does **not** help:
`start_microphone` spawns a thread and returns, so the HAL calls race whatever
the call order is.

## Why it is worse than a failure

`audio/mod.rs` names leg independence as one of three load-bearing properties:
"the system-audio leg failing must not stop the microphone, and neither may
stop the recording." That holds for a leg that *fails*. This is a leg that
**succeeds** and hangs the other one — a case the design does not cover, and
the failure mode is the worst available: capture reports healthy, both legs
report started, and the Meeting is recorded with no audio and nothing to say
why.

It also worked on 2026-09-01 with the same code (mic 5730 ms, peak 0.073, tap
running and silent), so it is state-dependent rather than deterministic. The
machine has a `Microsoft Teams Audio` virtual device, and `coreaudiod` had been
up 16 days.

## What to fix, in the order the value falls

1. **A leg that starts and never delivers must be detected.** Independent of
   the root cause, and the highest value: a recording that produces nothing is
   currently indistinguishable from a quiet room. A watchdog — no frames from a
   leg that claimed to start, within a few seconds — turns silence into a
   `Degraded` reason that reaches `audio_notes` and the Client.
2. **Do not let the tap block the microphone.** Options worth measuring:
   build the microphone stream to completion *before* creating the tap (needs
   `start_microphone` to signal readiness rather than returning as soon as the
   thread is spawned); create the tap on its own thread; or drop the aggregate
   device between Meetings.
3. **Reproduce it deliberately.** A test that starts the tap and then opens an
   input stream would have caught this, and can run on any machine with an
   input device. It hangs when it fails, so it needs a timeout.

## Note for whoever picks this up

`audio-check` could not see any of this until 2026-09-05: `init_tracing()` was
called only by the `daemon` subcommand, so the diagnostic command discarded
every log line the capture path emits. That is fixed; without it this bug is
invisible.
