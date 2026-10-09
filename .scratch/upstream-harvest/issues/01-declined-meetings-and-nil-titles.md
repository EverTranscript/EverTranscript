# 01: A declined meeting does not arm, and a nil EventKit title does not panic

Status: done, except the hand check on a real calendar named below

Approved as `UH-2026-10-02` (both items), 2026-10-06. Both live in
`crates/evertranscript-core/src/detect/calendar.rs`, in the same reader.

**1. Declined meetings.** `changes` armed every in-progress, non-all-day
event, declined ones included. An armed declined meeting announced a
recording for a meeting the Operator said they would skip, sent the
"nothing is recording" follow-up two minutes later, and, when it overlapped
the call they did join, could name that Meeting: `claim_armed_event` takes
the first armed event by id. anarlog added a declined skip in `b7586718d5`.

**2. Nil titles.** `event.title().to_string()` would panic on a nil title:
objc2-event-kit 0.3.2 types `title` as non-null, and the SDK header
(`EKCalendarItem.h`) marks it `null_unspecified`. anarlog's crash
ANARLOG-1T52 was a nil title on a calendar source; its fix, `d5fa9979b8`,
reads every EventKit title as nullable, events included.

- [x] `Reading` carries `declined`; `changes` skips it beside the all-day skip
- [x] macOS: organizer-is-current-user is never declined; otherwise the
  current-user attendee's `participantStatus() == Declined`
- [x] Windows: `Appointment::UserResponse() == Declined`, with
  `AppointmentProperties::UserResponse()` added to the fetch list (original
  code: anarlog has no Windows calendar)
- [x] macOS title read through `msg_send!` as nullable, nil as empty, into the
  existing "Untitled event" fallback
- [x] Tests: `a_declined_meeting_never_arms`, `declining_an_armed_meeting_ends_it`
- [x] `PORTS.md` rows and the attribution header; the strict gate recorded
  under "Not ported, deliberately"
- [ ] **Hand check, not done:** one declined test event on a Mac with calendar
  access, confirmed not to arm. No CI host has a calendar store, so the
  EventKit and WinRT property reads are untested by anything automatic. The
  nil-title path cannot be reached from a test either: no API mints a nil-title
  `EKEvent`
- [x] **Windows half compiled in place, 2026-10-09:** CI run 37981072811
  (d2c172e) built and tested it natively on `windows-latest`, and it passed.
  Before that: CI runs on `main` and pull
  requests only, and this branch has neither; the workspace cannot
  cross-compile (`scripts/check.sh`: `mp3lame-sys`). The WinRT lines were
  type-checked on their own instead: a scratch crate on `windows = "=0.62.2"`
  with the same features passes `cargo clippy --target x86_64-pc-windows-msvc
  -- -D warnings`.

Gate, 2026-10-06 on mac-mini-m6, as separate steps: `cargo fmt --all
--check`, `cargo build --workspace --all-targets` and `cargo clippy
--workspace --all-targets -- -D warnings` pass; `cargo test --workspace
--no-fail-fast` passes 1001 of 1002. The one failure is
`detect::macos::tests::a_real_microphone_hold_is_visible_to_the_detector`,
the host condition DECISIONS Q296 records — a freshly built test binary
holds no microphone grant here — in a file this change does not touch. With
the declined skip removed, both new tests fail. Decisions: Q300, Q301.
