# 03: Capture vertical — audio to disk

**What to build:** A recording captures real dual-channel audio into the Meeting from ticket 02: microphone + system audio behind the AudioSource seam (live and fixture implementations), absolute capture timestamps on one shared clock, and the ffmpeg checkpoint sink producing a crash-safe stereo AAC file in `.data/audio/`. The ADR-0029 churn contract works: device swaps continue the same Meeting, the system leg self-heals, and a killed Core leaves a recoverable recording.

**Blocked by:** 01, 02.

**Status:** done — system-audio capture verified as far as this machine's permissions allow

- [x] AudioSource trait with live and fixture implementations — the ratified seam; all end-to-end tests feed through it
- [x] macOS: CoreAudio process tap + private aggregate device (14.4+) for system audio, cpal for mic; Windows: WASAPI
      loopback + capture. Verified live on macOS 26.6: the tap is created, the aggregate device carries it, and the IO
      proc delivers 48 kHz mono at the correct rate. Deliberately **not** ScreenCaptureKit — that route would demand
      the Screen Recording permission for audio the narrower grant already covers (ADR-0027), and the guarantee suite
      fails the build if it is ever linked. The aggregate device holds the tap and *nothing else*: adding the output
      device as a sub-device records the same audio twice, an echo subtle enough that Meetily shipped it. Windows
      loopback is cpal building an input stream on an output device; it compiles but is unverified — no Windows here.
- [x] A refused audio-recording permission is detected rather than recorded. macOS grants the tap either way and then
      delivers digital silence forever, so a created tap proves nothing. The tell is that a global tap's callback only
      fires while something plays: frames arriving steadily while no sample is ever non-zero means audio is being
      played and we are being handed zeros. Confirmed on this machine, which has no grant.
- [x] Dual-channel frames carry absolute timestamps; the audio file and (future) transcript share the one clock — gaps are explicit, never silent drift
- [x] ffmpeg encodes 30s checkpoint files, lossless-concat merged at finalize into one stereo `.m4a` (L=mic, R=system) under `.data/audio/<id8>.m4a`
- [x] `kill -9` mid-recording → next start recovers checkpoints into a playable partial file and flags the Meeting recovered
- [x] Injected default-device change → budget-free capture respawn inside the same Meeting, gap accounted on the shared clock
- [x] Injected stream error → budgeted respawn with backoff; budget exhausted → ordered meltdown finalizes the Meeting cleanly
- [x] System-audio failure self-heals with bounded backoff while the mic leg keeps flowing
- [x] No pre-trigger/pre-roll buffering exists anywhere (ADR-0024 as amended)
