# 05: Windows detection — process enumeration and audio-session microphone state

**What to build:** The live DetectionSource for Windows: Win32 process and window enumeration plus audio-session microphone state, requiring no permission grant. **This is the ship gate** (ADR-0025 as amended, ADR-0030): the Windows column cannot be hollow.

**Blocked by:** 01, 02.

Status: blocked on hardware — builds and its tests pass on real Windows in CI; the detector's live behaviour is unobserved, and reading found three meeting rows that could not have matched there at all

- [x] Process enumeration for the Watchlist's exe entries, and audio-session state for the microphone condition — no permission prompt required on this platform
- [x] The exe→app table twin of the macOS helper table. **This was checked off while the table did not exist**, and the mistake was structural rather than careless: the macOS helper rule (strip at `.helper`) is genuinely platform-neutral code, so "the twin" looked satisfied by the function being shared. It was not — a twin of a *table* is a table, and `WINDOWS_EXECUTABLES` is now it. See the open criterion below for what that omission cost.
- [x] Poll-and-debounce rather than a port of the prior art's backoff, matching the macOS side; the shipped reference was not on the machine. Original criterion: taken from the shipped prior art (`mic_monitor_v2`) rather than improvised. **anarlog's Windows detector is a no-op stub** — the absorption catalog says so explicitly, and it must not be mistaken for a reference implementation
- [x] Browser Meetings works here too: any browser holding a hot microphone, helpers attributed to the responsible app
- [x] The same DetectionSource contract as macOS, proven by the same seam tests running on both targets in CI — a per-platform dialect of the trait is a failure of this ticket
- [x] **Compiles as part of the real workspace for `x86_64-pc-windows-msvc`, and passes `clippy -D warnings` there.** Not an isolated crate any more: `cargo-xwin` plus LLVM cross-builds the whole thing on this Mac, and `scripts/check.sh` runs it whenever the toolchain is present. It immediately found two unused imports that exist only on Windows — the class of defect a macOS-only loop cannot see at all.
- [x] **It runs on real Windows.** `windows-latest` in CI compiles, links and passes the whole suite, which took six rounds of fixing genuine Windows defects — a WinRT call with no apartment, an API needing package identity, one global pipe for every Core, cpal aborting where no audio device exists, and a CRLF checkout breaking the drift fixtures. Every one of those was invisible from macOS.
- [ ] **Awaiting the Operator, who has the machine — and one defect was found by reading while waiting.** Chasing the live Teams failure on macOS (ticket 09) exposed the same shape here, without a Windows box: the detector reports a lowercased executable name, `Watchlist::watches` compares ids exactly, and the shipped rows for Zoom, Teams and VooV are macOS *bundle ids*. **Three of the four meeting rows could not have matched anything on Windows** — on the platform ADR-0025 makes a ship gate, in a build CI calls green. Browsers were spared by accident, because `known_browsers` lists executables beside bundle ids. `WINDOWS_EXECUTABLES` now maps them, and the executable names in it are **unverified**: they are the one thing in this repo asserted from memory rather than read off a machine, which is exactly the mistake Teams and Safari both were. A wrong name matches nothing, i.e. fails as today does, so the table is safe to ship while it waits. `windows-check.md` asks for the real names by name. The rest stands: CI proves the code is sound, not that it detects, because that runner has neither microphone nor browser; the two-Cores-at-once case was fixed blind and nobody has watched it; and the per-browser matrix from ticket 09 has to be run there, not extrapolated from macOS

## How far the Windows build is actually verified, and where it stops

`scripts/check.sh` cross-builds and lints the real workspace for
`x86_64-pc-windows-msvc` (cargo-xwin + LLVM + Ninja) whenever the toolchain
is present. That is `check` and `clippy -D warnings`, both green, and it
found two Windows-only unused imports on its first run.

**`cargo xwin build` and `test` stop short, and not because of this code.**
Linking fails on a missing `ggml-blas`, and the cause is upstream: whisper-rs-sys's
build script asks for it under

```rust
if cfg!(target_os = "macos") || cfg!(feature = "openblas")
```

and `cfg!` inside a *build script* evaluates for the **host**, not the
target. Cross-compiling from a Mac therefore requests a library the Windows
cmake build never produced. On `windows-latest` the host is Windows, the
condition is false, and the link is never attempted — so **real CI is not
affected by this**, and nobody should read the cross-link failure as the
Windows build being broken.

What remains genuinely impossible here is execution. Wine is not a Windows
machine and cannot serve WASAPI audio sessions or the WinRT appointment
store, which are the only two things this ticket is really about.
