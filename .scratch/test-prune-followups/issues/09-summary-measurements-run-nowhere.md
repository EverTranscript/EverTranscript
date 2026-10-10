# 09: The two 4B summary measurements run in no CI job

Status: done: both ungated (DECISIONS Q312)

Found while doing ticket 07 (DECISIONS Q308).

`tests/summary_quality.rs` and `tests/summary_ninety_minutes.rs` start with
`#![cfg(unix)]`. CI sets `EVERTRANSCRIPT_SUMMARY_MODEL` and
`EVERTRANSCRIPT_MEASURE_SUMMARY_QUALITY` on Windows only, because a macOS
runner cannot drive the 4B (DECISIONS Q59, 5fe121b). So on macOS both files
skip, and on Windows they are not compiled. Neither has run in any CI job since
2026-09-01, and each green CI run says nothing about them.

The gate has no platform reason: `summary_quality.rs` even has a
`cfg!(windows)` branch to name the sidecar binary.

## Decide

- **Ungate both:** they run on the Windows job, which already loads the 4B.
  Measure the added time against the 55-minute limit first. `summary_quality`
  is allowed to fail when the model is bad, and the ninety-minute case recorded
  the reduce losing commitments (Q61), so check what each one asserts before
  turning it on.
- **Keep them manual:** say so in each file's header, and stop implying CI
  covers them.

## Answer

**Both ungated.** On Windows, `summary_quality` now runs the measurement CI
already asks for. `summary_ninety_minutes` adds only its two fixture checks
there, because its generation has its own switch,
`EVERTRANSCRIPT_MEASURE_NINETY_MINUTES`, that no workflow sets.

Cost: `summary_inference` took 267 s on Windows in run 37984670056, and the
Tests step took about 23 of its 55 minutes. On PR #1 (run 37998052049) the
quality suite loaded the 4B and passed all 8 checks in 140 s on Windows.

## Comments
