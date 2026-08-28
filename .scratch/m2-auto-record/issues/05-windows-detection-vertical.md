# 05: Windows detection — process enumeration and audio-session microphone state

**What to build:** The live DetectionSource for Windows: Win32 process and window enumeration plus audio-session microphone state, requiring no permission grant. **This is the ship gate** (ADR-0025 as amended, ADR-0030): the Windows column cannot be hollow.

**Blocked by:** 01, 02.

Status: blocked on hardware — compiles and lints clean for the target; never executed

- [x] Process enumeration for the Watchlist's exe entries, and audio-session state for the microphone condition — no permission prompt required on this platform
- [x] The exe→app table twin (the rule, derived rather than ported — see ticket 02) of the macOS helper table, ported as seed data with attribution and a `PORTS.md` entry
- [x] Poll-and-debounce rather than a port of the prior art's backoff, matching the macOS side; the shipped reference was not on the machine. Original criterion: taken from the shipped prior art (`mic_monitor_v2`) rather than improvised. **anarlog's Windows detector is a no-op stub** — the absorption catalog says so explicitly, and it must not be mistaken for a reference implementation
- [x] Browser Meetings works here too: any browser holding a hot microphone, helpers attributed to the responsible app
- [x] The same DetectionSource contract as macOS, proven by the same seam tests running on both targets in CI — a per-platform dialect of the trait is a failure of this ticket
- [x] **Compiles as part of the real workspace for `x86_64-pc-windows-msvc`, and passes `clippy -D warnings` there.** Not an isolated crate any more: `cargo-xwin` plus LLVM cross-builds the whole thing on this Mac, and `scripts/check.sh` runs it whenever the toolchain is present. It immediately found two unused imports that exist only on Windows — the class of defect a macOS-only loop cannot see at all.
- [ ] **Still blocked on hardware:** The code is typechecked against `x86_64-pc-windows-msvc` and has never been run: it was written on an Apple Silicon Mac. The crate itself cannot be cross-checked — `ring` needs an MSVC C toolchain — so the API usage was verified in an isolated crate against the real target and then ported in. Verification needs a real Windows 10+ x64 machine with the per-browser matrix from ticket 09 run there, not extrapolated from macOS

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
