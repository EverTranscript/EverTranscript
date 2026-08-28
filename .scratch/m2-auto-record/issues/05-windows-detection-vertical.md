# 05: Windows detection — process enumeration and audio-session microphone state

**What to build:** The live DetectionSource for Windows: Win32 process and window enumeration plus audio-session microphone state, requiring no permission grant. **This is the ship gate** (ADR-0025 as amended, ADR-0030): the Windows column cannot be hollow.

**Blocked by:** 01, 02.

Status: ready-for-human

- [x] Process enumeration for the Watchlist's exe entries, and audio-session state for the microphone condition — no permission prompt required on this platform
- [x] The exe→app table twin (the rule, derived rather than ported — see ticket 02) of the macOS helper table, ported as seed data with attribution and a `PORTS.md` entry
- [x] Poll-and-debounce rather than a port of the prior art's backoff, matching the macOS side; the shipped reference was not on the machine. Original criterion: taken from the shipped prior art (`mic_monitor_v2`) rather than improvised. **anarlog's Windows detector is a no-op stub** — the absorption catalog says so explicitly, and it must not be mistaken for a reference implementation
- [x] Browser Meetings works here too: any browser holding a hot microphone, helpers attributed to the responsible app
- [x] The same DetectionSource contract as macOS, proven by the same seam tests running on both targets in CI — a per-platform dialect of the trait is a failure of this ticket
- [ ] **Not done, and the reason this ticket is `ready-for-human`.** The code is typechecked against `x86_64-pc-windows-msvc` and has never been run: it was written on an Apple Silicon Mac. The crate itself cannot be cross-checked — `ring` needs an MSVC C toolchain — so the API usage was verified in an isolated crate against the real target and then ported in. Verification needs a real Windows 10+ x64 machine with the per-browser matrix from ticket 09 run there, not extrapolated from macOS
