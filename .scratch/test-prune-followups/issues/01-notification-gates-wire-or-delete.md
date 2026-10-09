# 01: Notification `Gates`, its catalog and `do_not_disturb`: wire them in or delete them

Status: done: kept unwired; `do_not_disturb` has a test (DECISIONS Q305)

Found by the test prune of 2026-10-09 (DECISIONS Q302–Q304, commit 0e95de2).

`crates/evertranscript-core/src/detect/notify.rs` holds code that nothing in
production calls:

- `Gates` (`:85`): the cooldown, the silenced-app list and the "nothing while
  recording" rule.
- `catalog` (`:33`): the notification strings, in two locales.
- `do_not_disturb()` (`:152` macOS, `:166` elsewhere): reads
  `~/Library/DoNotDisturb/DB/Assertions.json`.

Production builds `SilentNotifier` (`lib.rs:126`), which says nothing. Only the
unit tests in `notify.rs` reach the three items above.

The prune kept the `Gates` and catalog tests (Q303: code that is built but not
yet wired keeps its tests). It deleted one assertion-free probe of
`do_not_disturb`, so `do_not_disturb` now has no test at all.

## Decide

- **Wire it in:** a real `Notifier` uses `Gates`, the catalog and
  `do_not_disturb`. Then add a test for `do_not_disturb` that writes a fake
  `Assertions.json` (on and off) and checks the answer. Its rule is "fail
  towards notifying", so a missing or unreadable file must give `false`.
- **Delete it:** remove `Gates`, `catalog`, `do_not_disturb` and their tests,
  and keep `Notifier` and `SilentNotifier`.

## Answer

**Kept, unwired.** `do_not_disturb` now reads its file through
`focus_is_on(path)`, and `a_focus_check_that_cannot_answer_lets_the_notification_through`
checks the fail-open rule: a missing file and a file with no assertion record
both read as Focus off. Breaking the missing-file branch to `true` fails it.

**For whoever builds delivery:** do not wire `Gates` in as it is. Its
two-minute cooldown runs on the wall clock (`Instant::now()`), and the policy
sends the "nothing is recording" follow-up about two minutes after the heads-up,
timed on detection time. Wired as written, the cooldown after the heads-up can
swallow the follow-up, which is the notification that matters most. Choose
one clock first.

## Comments
