# 02: A microphone that drops mid-meeting comes back

Status: ready-for-human

Approved as `UH-2026-10-10` (one item), 2026-10-14, through the mac-mini-m6
session. Upstream is fastrepl/anarlog `693500220d` (#8048, ANLG-467): each
capture channel reopens on its own after 1s/2s/4s, skips the device that just
failed, counts five seconds with no frames as a failure, and counts
consecutive failures that reset only once frames flow again.

**The bug here.** `ChurnPolicy::decide` answers a `StreamFailed` with
`Action::RestartLeg`, and `audio/recorder.rs` answers that with a `debug!`:
"The source owns its own reconnection". `audio/live.rs` does not reconnect.
`run_microphone` opens one cpal stream and sleeps until stopped. cpal 0.15.3
on macOS answers a vanished device by stopping the stream and calling the
error callback once. So when AirPods run flat, Bluetooth drops, or a USB mic
is unplugged, the mic leg is silent for the rest of the Meeting. One
`StreamFailed` never reaches `EndLeg`, and the silent-leg watchdog only
judges legs that never delivered, so the Meeting looks complete. CONTEXT.md's
Leg entry forbids that ("marked partial — with the reason, in the record").
ADR-0029's amendment (ii) requires the respawn; M1 ticket 03 checked it off
against `FixtureSource`, which keeps playing after an injected failure.

**Scope: the mic leg only**, as the proposal's "The port" describes.

- [x] `audio/live.rs`: the mic leg becomes a reopen loop (upstream
  `run_channel`). The error callback and a stall of 5s after the leg's first
  frame end an attempt; it waits 1s/2s/4s and reopens the default input,
  skipping the failed device's name when another input exists
  (`new_excluding`). One microphone start in flight at a time
  (`StartupPermit`).
- [x] A reopen onto a different device sends `DeviceChanged { Mic }`, so
  `MicIsolation` stops claiming the mic was isolated.
- [x] `audio/supervisor.rs`: `RestartTracker` counts consecutive failures and
  resets on proof of health, as upstream's `record_consecutive_restart` /
  `reset`. Exhausting it ends the leg, and `EndLeg` writes the reason.
- [x] Tests with a fake opener: fail ×N then deliver; stall then recover;
  budget exhausted ends the leg.
- [x] `PORTS.md` rows and attribution headers; "Not ported, deliberately".
- [ ] **Hand check, not possible from this run:** record, switch off AirPods
  or pull a USB mic, keep talking into the built-in mic, and confirm the
  Transcript continues and the Meeting is not marked partial.

Done on branch `upstream-harvest/2026-10-14-mic-channel-recovery`; decisions
Q348–Q350. What is left is the hand check above, which needs a person and a
microphone to pull.

**Not ported:** the manual `retry_audio_capture` command and its float-bar
button (no such surface here), the joiner's `pop_available_pair` (ours fills
gaps from absolute stamps), and the system leg (a process tap delivers
nothing while nothing plays, so a 5s frame timeout would convict every quiet
stretch).
