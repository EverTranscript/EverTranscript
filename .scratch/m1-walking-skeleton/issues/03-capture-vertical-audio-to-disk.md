# 03: Capture vertical — audio to disk

**What to build:** A recording captures real dual-channel audio into the Meeting from ticket 02: microphone + system audio behind the AudioSource seam (live and fixture implementations), absolute capture timestamps on one shared clock, and the ffmpeg checkpoint sink producing a crash-safe stereo AAC file in `.data/audio/`. The ADR-0029 churn contract works: device swaps continue the same Meeting, the system leg self-heals, and a killed Core leaves a recoverable recording.

**Blocked by:** 01, 02.

**Status:** mostly done — system-audio capture outstanding

- [x] AudioSource trait with live and fixture implementations — the ratified seam; all end-to-end tests feed through it
- [~] macOS: CoreAudio process tap + private aggregate device (14.4+) for system audio, cpal for mic; Windows: WASAPI loopback + capture
      **Microphone capture is implemented (cpal, both platforms). System-audio capture is not** — the leg reports itself
      `Unavailable`, which the churn policy handles as "record the microphone and mark the audio partial". That
      degradation is tested; the platform work (CoreAudio process taps / WASAPI loopback) remains.
- [x] Dual-channel frames carry absolute timestamps; the audio file and (future) transcript share the one clock — gaps are explicit, never silent drift
- [x] ffmpeg encodes 30s checkpoint files, lossless-concat merged at finalize into one stereo `.m4a` (L=mic, R=system) under `.data/audio/<id8>.m4a`
- [x] `kill -9` mid-recording → next start recovers checkpoints into a playable partial file and flags the Meeting recovered
- [x] Injected default-device change → budget-free capture respawn inside the same Meeting, gap accounted on the shared clock
- [x] Injected stream error → budgeted respawn with backoff; budget exhausted → ordered meltdown finalizes the Meeting cleanly
- [x] System-audio failure self-heals with bounded backoff while the mic leg keeps flowing
- [x] No pre-trigger/pre-roll buffering exists anywhere (ADR-0024 as amended)
