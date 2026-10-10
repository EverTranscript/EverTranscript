# 01: Captions for a 53-second recording took 48 minutes on an i5-8400

Status: done

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

## Answer

**Cause: whisper.cpp was compiled without optimization on Windows.**
whisper-rs-sys's CMakeCache on windows-zx8 read
`CMAKE_C_FLAGS_RELEASE= -nologo -MD -Brepro -W0`, with no `/O` flag, so MSVC
built ggml at `/Od`. cmake-rs replaces CMake's Release flags for the Visual
Studio generator and drops every `/O` (rust-lang/cmake-rs#240).
llama-cpp-sys-2 adds `/O2` back itself, so the summarizer was fine.
whisper-rs-sys 0.15.0 does not. The AVX2 instructions in the binary were
real, but unoptimized code around them made the decode about 50 times
slower. This was true of v1.0.1 and of `main`.

**Fix (Q315):** on Windows, both workflows now set `CMAKE_C_FLAGS_RELEASE`
and `CMAKE_CXX_FLAGS_RELEASE` to `-O2 -Ob2 -DNDEBUG`. whisper-rs-sys passes
`CMAKE_*` through, and cmake-rs keeps a flag variable once it is set. The CI
Rust cache prefix is now `v2-rust`, because whisper-rs-sys does not rebuild
when these variables change. The CMakeCache guard in both workflows also
fails when either Release flag lacks `-O2`. Tested locally against five
cache shapes: unoptimized, optimized, C++ only unoptimized, native on, and
no cache.

**Measured on windows-zx8** (same 53 s recording, `main` at c8895d8):

| Build | Threads | Time per chunk | Stop to `audio finalized` |
|---|---|---|---|
| v1.0.1, unoptimized | 3 | not logged | 48 min 2 s |
| `main`, unoptimized | 3 | none done after 5 min | stopped at 5 min |
| `main`, `-O2` | 3 | 55.0–55.8 s | 4 min 12 s |
| `main`, `-O2`, every core (experiment) | 6 | 35.3–38.1 s | about 2 min 40 s |

Both legs transcribed correctly in the optimized run.

**What is left** is filed as ticket 03: each chunk costs the same whatever
its length (2 s and 25 s chunks both took about 55 s), so even optimized,
this CPU cannot keep up with two legs of live speech. The thread rule stays
at half the cores (Q316).

Local Windows builds outside CI are still unoptimized unless the developer
sets the same two variables.

## Comments
