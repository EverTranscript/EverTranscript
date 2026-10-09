# 09: The two 4B summary measurements run in no CI job

Status: needs-triage

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

## Comments
