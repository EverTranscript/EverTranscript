# 03: Capture vertical — audio to disk

**What to build:** A recording captures real dual-channel audio into the Meeting from ticket 02: microphone + system audio behind the AudioSource seam (live and fixture implementations), absolute capture timestamps on one shared clock, and the ffmpeg checkpoint sink producing a crash-safe stereo AAC file in `.data/audio/`. The ADR-0029 churn contract works: device swaps continue the same Meeting, the system leg self-heals, and a killed Core leaves a recoverable recording.

**Blocked by:** 01, 02.

**Status:** ready-for-agent

- [ ] AudioSource trait with live and fixture implementations — the ratified seam; all end-to-end tests feed through it
- [ ] macOS: CoreAudio process tap + private aggregate device (14.4+) for system audio, cpal for mic; Windows: WASAPI loopback + capture
- [ ] Dual-channel frames carry absolute timestamps; the audio file and (future) transcript share the one clock — gaps are explicit, never silent drift
- [ ] ffmpeg encodes 30s checkpoint files, lossless-concat merged at finalize into one stereo `.m4a` (L=mic, R=system) under `.data/audio/<id8>.m4a`
- [ ] `kill -9` mid-recording → next start recovers checkpoints into a playable partial file and flags the Meeting recovered
- [ ] Injected default-device change → budget-free capture respawn inside the same Meeting, gap accounted on the shared clock
- [ ] Injected stream error → budgeted respawn with backoff; budget exhausted → ordered meltdown finalizes the Meeting cleanly
- [ ] System-audio failure self-heals with bounded backoff while the mic leg keeps flowing
- [ ] No pre-trigger/pre-roll buffering exists anywhere (ADR-0024 as amended)
