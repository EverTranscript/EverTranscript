# 10: Electron Client vertical

**What to build:** The minimal Client, end to end: Meeting list, Meeting view with the transcript, live captions during recording, retitle and delete, mid-recording attach that loses nothing, and starts-the-Core-if-absent — all speaking the generated protocol types, concurrent with the CLI, with every string externalized.

**Blocked by:** 06.

**Status:** done — visually verified on a MacBook Air (M4) on 2026-08-28; launching with no Core running crashed the app until fixed

- [x] The app consumes only generated ts-rs protocol types — no hand-written wire types
- [x] Meeting list and Meeting view (title, metadata, transcript with channel labels) from the read surface
- [x] Live caption view via the lossy subscription; opening the app mid-recording shows transcript-so-far then live tail (snapshot-then-tail, story 24)
- [x] Retitle (renames the Mirror via the Core) and whole-Meeting delete work from the UI
- [x] Launching the Client with no Core running starts it; closing or killing the Client provably never affects an active recording
- [x] CLI commands work while the app is open (concurrent clients)
- [x] React 19 + Vite + Tailwind, strict TS; all user-facing strings externalized (English catalog)

## Visual verification, 2026-08-28

Never done before: the machine this was built on had no display and no
screen-recording permission, so every box above was checked from behaviour the
tests could reach. One of them was not true.

**Launching the Client with no Core running crashed the app.** `startCore` spawned
the bare name `evertranscript`, which is not on `PATH` — the binary only exists in
the checkout — and a missing binary reports ENOENT *asynchronously* rather than
throwing, so the `try/catch` around `spawn` never saw it. Unhandled, that became an
uncaught exception in the main process and Electron replaced the window with a
crash dialog. The claim in the box above was exactly inverted: rather than starting
the Core, launching without one killed the Client.

Fixed: the binary is resolved before spawning (`EVERTRANSCRIPT_BIN`, then `PATH`
searched by hand because a GUI app inherits a much smaller one than a shell, then
the checkout's own build), the child's `error` event is handled so it can never
reach the main process unhandled, and the reason reaches the renderer instead of
the generic "no Core is listening", which is true and useless when the real problem
is that the binary was never found.

Verified after the fix, against a running Client:

- launching with no Core starts one, and it is the Core the CLI then talks to;
- the Meeting list, Meeting view, title, metadata, and Rename/Delete all render;
- **the "This recording is incomplete" banner renders correctly**, with the reason
  as a bullet beneath it — the thing the M1 handoff asked to look at;
- the transcript shows timestamps and channel labels;
- CLI commands work while the app is open;
- killing the Client mid-recording leaves the Core in `Recording` and the Meeting
  completes normally (ADR-0026).

One quality problem is visible in the transcript and belongs with the other ASR
findings rather than here: segments consisting of a bare `.` reach the record, so
`filters::clean` is letting punctuation-only decodes through as speech.
