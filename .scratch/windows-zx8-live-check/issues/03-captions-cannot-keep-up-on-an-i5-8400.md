# 03: Even optimized, captions cannot keep up with live speech on an i5-8400

Status: needs-triage

Found while fixing ticket 01 on 2026-10-09 (Q315, Q316).

## What was measured

On windows-zx8 (Intel Core i5-8400, 6 cores, AVX2), with `main` at c8895d8
and whisper.cpp built with the Q315 flags, using the same 53 s two-leg
recording:

| Threads | Chunks (audio length) | Decode per chunk |
|---|---|---|
| 3 (the product rule) | 25 s, 25 s, 3 s, 25 s, 2 s | 55.0–55.8 s each |
| 6 (experiment) | 25 s, 25 s, 3 s, 25 s, 2.3 s | 35.3–38.1 s each |

- The cost per chunk does not depend on the chunk's length: whisper encodes a
  fixed 30 s window, so a 2 s chunk costs as much as a 25 s one. Here, two of
  the five chunks were 5 s of audio and took 40% of the decode time.
- At 3 threads, a 25 s chunk takes about 2.2 times its length to decode. The
  two legs are decoded separately.

## Why it matters

The worker's queue holds "roughly a minute of blocks" and drops what does
not fit (`audio/recorder.rs:367`–`:370`). A machine that decodes slower than
real time therefore loses captions in any meeting longer than a few
minutes, and reports "captions: N block(s) went untranscribed". This
follows from the numbers above and the queue rule. A long meeting was not
recorded to confirm it.

## Levers, cheapest to measure first

1. **Short chunks.** whisper.cpp's `audio_ctx` sizes the encoder to the
   audio instead of 30 s. Shrinking it for chunks well under 30 s would cut
   most of the cost of the 2–3 s chunks. It can cost accuracy on
   large-v3-turbo, so measure it with `tests/transcription_quality.rs`.
2. **Fewer chunks.** `ChunkPolicy` cuts at 20–25 s (`asr/vad.rs:126`). Chunks
   closer to 30 s pay for fewer windows per second of speech.
3. **Compiler.** ggml built with clang-cl instead of MSVC may decode faster.
   Measure it before changing the toolchain.
4. **Threads.** 6 threads was 1.5 times faster. The rule gives whisper half
   the cores, to keep headroom for capture and for the call app (Q316).
   Measure CPU headroom during a real call before changing it.
5. **A smaller model on slow CPUs.** This is a product decision about
   accuracy.

## How to measure

windows-zx8 is reachable as `ssh windows-zx8` from mac-mini-m6. Q315's run
used a git bundle of `main`, built with `GGML_NATIVE=OFF`,
`CMAKE_C_FLAGS_RELEASE` and `CMAKE_CXX_FLAGS_RELEASE` set, and
`EVERTRANSCRIPT_LOG=evertranscript=info,evertranscript_core=info,evertranscript_core::asr=debug`
so each `transcribed a chunk` line shows `elapsed_ms` and `audio_ms`. Run
it as a scheduled task, because the machine sleeps when idle and a dropped
SSH session ends the run. Pipe `y` into `evertranscript acknowledge`,
because a hidden window still counts as a terminal.

## Comments
