# 02: The Rust update check `updates::check`: wire it in or delete it

Status: done: deleted (DECISIONS Q306)

Found by the test prune of 2026-10-09 (DECISIONS Q302–Q304, commit 0e95de2).

`crates/evertranscript-core/src/updates.rs:69` (`check`) asks GitHub for the
latest release. Nothing in production calls it. The Electron client does the
real update check in `clients/electron/src/main/updates.ts`. The only use of
the module outside itself is `posture.rs:163`, which reads
`UPDATE_FEED_HOST` to declare the host as sanctioned traffic.

The prune kept its tests (Q303). One of them,
`an_unreachable_feed_is_not_an_error_the_operator_must_dismiss` (`:166`),
makes a real HTTPS call to GitHub each time the suite runs, and accepts either
result. So it is slow, it needs a network, and it cannot fail on the result it
names.

## Decide

- **Wire it in:** the Core reports `UpdateStatus` to Clients. Then replace the
  network test with one that points `check` at a local or unreachable address.
- **Delete it:** remove `check`, `UpdateStatus` and their tests. Keep
  `UPDATE_FEED_HOST` while the Electron client still contacts that host,
  because `posture.rs` declares it.

## Answer

**Deleted.** `check`, `UpdateStatus`, `UPDATE_FEED_PATH`, `CHECK_TIMEOUT` and
their three tests are gone, along with the test that called GitHub on every
run. `UPDATE_FEED_HOST` stays, because the trust surface shows it. Two comments
in `clients/electron/src/main/` no longer mention a check in the Core.

## Comments
