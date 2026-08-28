# 10: Electron Client vertical

**What to build:** The minimal Client, end to end: Meeting list, Meeting view with the transcript, live captions during recording, retitle and delete, mid-recording attach that loses nothing, and starts-the-Core-if-absent — all speaking the generated protocol types, concurrent with the CLI, with every string externalized.

**Blocked by:** 06.

**Status:** ready-for-agent

- [ ] The app consumes only generated ts-rs protocol types — no hand-written wire types
- [ ] Meeting list and Meeting view (title, metadata, transcript with channel labels) from the read surface
- [ ] Live caption view via the lossy subscription; opening the app mid-recording shows transcript-so-far then live tail (snapshot-then-tail, story 24)
- [ ] Retitle (renames the Mirror via the Core) and whole-Meeting delete work from the UI
- [ ] Launching the Client with no Core running starts it; closing or killing the Client provably never affects an active recording
- [ ] CLI commands work while the app is open (concurrent clients)
- [ ] React 19 + Vite + Tailwind, strict TS; all user-facing strings externalized (English catalog)
