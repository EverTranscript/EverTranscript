# 03: The score helpers `margin_trials` and `curve`: use them or delete them

Status: done: deleted (DECISIONS Q307)

Found by the test prune of 2026-10-09 (DECISIONS Q302–Q304, commit 0e95de2).

`crates/evertranscript-core/src/diarize/score.rs` has `margin_trials` (`:633`),
`Operating` (`:665`) and `curve` (`:683`). They turn scored trials into the
evidence a match *margin* is chosen from. Their only callers are their own unit
tests (`score.rs:1166`–`:1241`). No accuracy test, script or production path
calls them.

The prune kept their tests (Q303).

## Decide

- **Use them:** `tests/diarization_accuracy.rs` prints a margin curve from
  its labelled voices, so the next retune of `MATCH_MARGIN` has measured
  evidence.
- **Delete them:** remove the three items and their four tests.

## Answer

**Deleted.** `margin_trials`, `Operating`, `curve` and their four tests are
gone. Nothing outside those tests had called them since a83e6fa, and
`MATCH_MARGIN` was chosen through the matcher grid instead.

## Comments
