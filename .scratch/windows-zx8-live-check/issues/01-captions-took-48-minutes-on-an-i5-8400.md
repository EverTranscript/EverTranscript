# 01: Captions for a 53-second recording took 48 minutes on an i5-8400

Status: needs-triage

Found during the live v1.0.1 check on windows-zx8 on 2026-10-09 (Q313).

## What happened

- Machine: windows-zx8, an Intel Core i5-8400 (6 cores, AVX2, no AVX-512).
- Build: the v1.0.1 Core from the release installer, run as
  `evertranscript daemon` with its own History and app-support folders.
- Recording: 53 s, microphone and WASAPI loopback, while a 27 s spoken
  clip played through the speakers.
- Core log, UTC: the recording stopped at 22:51:36. `audio finalized` came
  at 23:39:38, and diarization finished at 23:39:40.
- `audio finalized` comes only after the transcription worker drains its
  queue: `audio/recorder.rs:307` (`worker.finish().await`) runs before
  `:323` (`sink.finalize()`). So the 48 minutes was transcription catching
  up.
- No captions were lost: the log has no "transcription fell behind"
  warning, and the transcript was correct.
- whisper ran with `threads=3`. `asr/whisper.rs:81` takes half the
  available cores, clamped to 2–8.
- For about 2 minutes after the stop, the Core was also downloading and
  checking the 4B summary model, which was ready at 22:52:58. After that it
  logged nothing until 23:39.

At this rate, about 54 times the recording's length, a one-hour meeting
would take more than two days to caption.

## Ruled out

- A build without SIMD: the v1.0.1 Core has ggml's AVX2 kernels (296
  `vpsignb ymm` and 1,521 `vpmaddubsw ymm` instructions).
- A capture hang: the loopback thread checks its stop flag every 20 ms
  (`audio/system/windows.rs:128`). Finalization waits on transcription, not
  on capture.

## Not known

- Whether `main` is this slow. v1.0.1 is from 2026-09-06.
- Whether something else kept the machine busy.
- Where the time goes. Candidates: the cost of each block's decode, short
  blocks padded to whisper's 30 s window, both legs decoded, or only 3
  threads.

## Do

1. Get a `main` build of the Core onto windows-zx8. It is reachable as
   `ssh windows-zx8` from mac-mini-m6.
2. Fetch the models first, so no download competes with the run. Set
   `EVERTRANSCRIPT_NO_LOGIN_ITEM=1`, so the test copy does not register
   itself to start at login.
3. Repeat the same recording. Note the time from the stop to
   `audio finalized`, and the Core's CPU time.
4. If `main` is still slow, log each block's decode time and try 6 threads.

## Comments
