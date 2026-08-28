# 10: Electron Client vertical

**What to build:** The minimal Client, end to end: Meeting list, Meeting view with the transcript, live captions during recording, retitle and delete, mid-recording attach that loses nothing, and starts-the-Core-if-absent — all speaking the generated protocol types, concurrent with the CLI, with every string externalized.

**Blocked by:** 06.

**Status:** done — not visually verified (no reachable GUI session on this machine)

- [x] The app consumes only generated ts-rs protocol types — no hand-written wire types
- [x] Meeting list and Meeting view (title, metadata, transcript with channel labels) from the read surface
- [x] Live caption view via the lossy subscription; opening the app mid-recording shows transcript-so-far then live tail (snapshot-then-tail, story 24)
- [x] Retitle (renames the Mirror via the Core) and whole-Meeting delete work from the UI
- [x] Launching the Client with no Core running starts it; closing or killing the Client provably never affects an active recording
- [x] CLI commands work while the app is open (concurrent clients)
- [x] React 19 + Vite + Tailwind, strict TS; all user-facing strings externalized (English catalog)
