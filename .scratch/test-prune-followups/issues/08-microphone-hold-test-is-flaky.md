# 08: `a_real_microphone_hold_is_visible_to_the_detector` fails intermittently

Status: done: the test was wrong; it now skips without a microphone (DECISIONS Q309)

Seen during the test prune of 2026-10-09, on mac-mini-m6.

`crates/evertranscript-core/src/detect/macos.rs:305` opens a real capture stream
and polls `recording_process_count()` every 200 ms for up to five seconds. It
expects CoreAudio to report at least one process recording. It skips only when the machine cannot
open a microphone at all.

It failed in the baseline run at `7edcc2e`, before any test was removed. It also
failed in one later run, and passed in another, with no change to the code it
covers. So the cause is the machine or the timing, not the prune.

## To find out

- Does it fail when it runs alone (`cargo test -p evertranscript-core
  a_real_microphone_hold`), or only when the suite runs in parallel?
- Does another app holding the microphone, or a privacy prompt, change the
  result?
- Does the macOS CI runner pass it or skip it?

Then decide: fix the timing, make it skip when the result cannot be trusted,
or mark it `#[ignore]` and run it by hand.

## Answer

**The test, not the timing.** Run alone it failed 5 of 5 times. mac-mini-m6
has no input device. `LiveSource::start` returns Ok when either leg starts, and
the system-audio tap starts here, so the skip never fired and the test waited
for a microphone hold that could not happen. It passed only when another test
in the parallel suite happened to be capturing input.

The test now calls `start_microphone_only`, which fails without a microphone,
so it skips (3 of 3 runs here). On a machine with a microphone it checks the
same thing as before.

## Comments
