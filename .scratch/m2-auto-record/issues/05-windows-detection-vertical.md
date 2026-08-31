# 05: Windows detection — process enumeration and audio-session microphone state

**What to build:** The live DetectionSource for Windows: Win32 process and window enumeration plus audio-session microphone state, requiring no permission grant. **This is the ship gate** (ADR-0025 as amended, ADR-0030): the Windows column cannot be hollow.

**Blocked by:** 01, 02.

Status: closed. Live Windows testing ended on the Operator's call 2026-08-31, having found and fixed a platform that had never detected anything; the capture-endpoint gap that run exposed is fixed too. Standing risks are named in the criteria rather than left as open work.

- [x] Process enumeration for the Watchlist's exe entries, and audio-session state for the microphone condition — no permission prompt required on this platform
- [x] The exe→app table twin of the macOS helper table. **This was checked off while the table did not exist**, and the mistake was structural rather than careless: the macOS helper rule (strip at `.helper`) is genuinely platform-neutral code, so "the twin" looked satisfied by the function being shared. It was not — a twin of a *table* is a table, and `WINDOWS_EXECUTABLES` is now it. See the open criterion below for what that omission cost.
- [x] Poll-and-debounce rather than a port of the prior art's backoff, matching the macOS side; the shipped reference was not on the machine. Original criterion: taken from the shipped prior art (`mic_monitor_v2`) rather than improvised. **anarlog's Windows detector is a no-op stub** — the absorption catalog says so explicitly, and it must not be mistaken for a reference implementation
- [x] Browser Meetings works here too: any browser holding a hot microphone, helpers attributed to the responsible app
- [x] The same DetectionSource contract as macOS, proven by the same seam tests running on both targets in CI — a per-platform dialect of the trait is a failure of this ticket
- [x] **Compiles as part of the real workspace for `x86_64-pc-windows-msvc`, and passes `clippy -D warnings` there.** Not an isolated crate any more: `cargo-xwin` plus LLVM cross-builds the whole thing on this Mac, and `scripts/check.sh` runs it whenever the toolchain is present. It immediately found two unused imports that exist only on Windows — the class of defect a macOS-only loop cannot see at all.
- [x] **It runs on real Windows.** `windows-latest` in CI compiles, links and passes the whole suite, which took six rounds of fixing genuine Windows defects — a WinRT call with no apartment, an API needing package identity, one global pipe for every Core, cpal aborting where no audio device exists, and a CRLF checkout breaking the drift fixtures. Every one of those was invisible from macOS.
- [x] **Run on the Operator's machine, 2026-08-31, Windows 11 Pro 26200 — and Windows detection had never worked at all.** Not a wrong name this time: no name. `executable_name` asked PSAPI's `GetModuleBaseNameW` over a handle opened `PROCESS_QUERY_LIMITED_INFORMATION`, a right that call is not documented against; it returned `ERROR_ACCESS_DENIED` for every process on the machine — Edge, Teams, and the Core itself — and a zero return is indistinguishable there from "no such process". `microphone_holders` therefore answered "nobody" while Edge plainly held the microphone. Every Windows row was unreachable: the browsers, `WINDOWS_EXECUTABLES`, all of it. The fix is `QueryFullProcessImageNameW`, which is documented against the right the detector asks for. **The prediction above was right that a fifth defect was waiting and wrong about where** — the table was never reached, so its names were never the thing standing between this platform and working. Observed after the fix: Edge starts a Meeting as `app="msedge.exe"` and stops it after the release debounce; two Cores with different runtime dirs bind `evertranscript-soulm` and `evertranscript-soulm-fb9cc279f571e5ef` and answer `status` independently; the calendar logs `no calendar access…` and does not crash; the Watchlist seeds all five rows. A packaged (MSIX) process names correctly too, which is the shape Teams is. What follows is what this criterion said while it waited, kept because it is the record of the reasoning: **Awaiting the Operator, who has the machine — and one defect was found by reading while waiting.** Chasing the live Teams failure on macOS (ticket 09) exposed the same shape here, without a Windows box: the detector reports a lowercased executable name, `Watchlist::watches` compares ids exactly, and the shipped rows for Zoom, Teams and VooV are macOS *bundle ids*. **Three of the four meeting rows could not have matched anything on Windows** — on the platform ADR-0025 makes a ship gate, in a build CI calls green. Browsers were spared by accident, because `known_browsers` lists executables beside bundle ids. `WINDOWS_EXECUTABLES` now maps them. The names in it were written from memory first — the exact mistake Teams and Safari both were — and then checked against the equivalent table in Granola's shipped bundle, the source the absorption catalog names for this. **Three survived and one did not**: VooV was written `wemeetapp.exe → com.tencent.meeting` and is really `voovmeetingapp.exe → com.tencent.tencentmeeting`, wrong in both halves. Only identifiers for rows this product already watches were taken. That is a stronger provenance than memory and still not observation: it establishes what the names are, not that the detector reports them. A wrong name matches nothing, i.e. fails as today does, so the table is safe to ship while it waits. The Chinese 腾讯会议 executable is still unknown and deliberately not guessed at. `windows-check.md` asks for the real names by name. The rest stands: CI proves the code is sound, not that it detects, because that runner has neither microphone nor browser; the two-Cores-at-once case was fixed blind and nobody has watched it; and the per-browser matrix from ticket 09 has to be run there, not extrapolated from macOS
- [x] **Live Windows testing is closed, on the Operator's call (2026-08-31), with what it did not reach named rather than erased.** The run proved the platform works: the ACCESS_DENIED defect above was found and fixed, Edge starts and stops a Meeting as `app="msedge.exe"`, two Cores coexist, the calendar declines without crashing, and a packaged MSIX process names correctly. What it could not reach is meeting apps: that machine has Edge and Teams and nothing else — no Zoom, VooV, 腾讯会议, Chrome, Firefox, Brave, Opera or Arc — and Teams was never driven into a call. So `WINDOWS_EXECUTABLES` stays checked-against-a-competitor's-table but unobserved, and the ticket-09 browser matrix has one Windows row filled.

  **This closes as a decision, not as a claim that the matrix is complete.** The Operator has the machine and has ended this line of work; asking for more of their time on it is theirs to offer. The residual below is carried forward as standing risk rather than pending M2 work, and it is deliberately specific so that a later failure is recognised instead of re-derived:

  - **Teams on Windows is the live risk.** Teams 2.x there is WebView2-hosted — one `ms-teams.exe` beside 24 `msedgewebview2.exe` children — so the process owning its capture session may be `msedgewebview2.exe`, not the row's `ms-teams.exe`. That is the same shape as `com.microsoft.teams2.modulehost`, which Teams already produced once on macOS. **Do not add `msedgewebview2.exe` on the strength of that sentence**: it would match every WebView2 app, and inventing the name is the mistake this milestone is a record of. `examples/mic-holders.rs` answers it in one command whenever a signed-in Teams call is available.
  - **腾讯会议's executable is still unknown**, and still not guessed at.
  - Zoom, VooV and the five unrun browsers are unobserved on Windows.

- [x] **The detector now reads every active capture endpoint.** It asked `GetDefaultAudioEndpoint(eCapture, eMultimedia)`, which made two false-negative sources at once. The one the probe observed: an app recording from a second microphone was invisible. The sharper one, which the run did not test: Windows keeps a *separate* default per `ERole` and points communications software at `eCommunications` — and it reassigns that role by itself when a headset appears, so the roles routinely disagree and the detector was liable to be watching the endpoint the meeting was not on. A headset was enough; a second microphone was never required. `EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)` subsumes both and removes the need to guess which role an app chose.

  Two deliberate choices in the shape of it. **Failures are per-device and never fatal**, because the defect this platform shipped with was a call that failed and looked exactly like an idle machine — one unreadable endpoint must not be able to blank the whole answer. And a machine that offers no endpoints **says so at debug** rather than answering "nobody" in silence, which is the same postmortem again.

  **Cross-compiles clean under `clippy -D warnings` for `x86_64-pc-windows-msvc`, and that is worth exactly what it was worth last time — nothing about runtime.** What makes it better founded than the code it replaces is that the enumeration is lifted from `examples/mic-holders.rs`, which ran on the Operator's real machine and printed the endpoints this now reads. **Unobserved on Windows in the detector itself**, and the honest next step is one `mic-holders` run against a headset, which would also settle whether the two roles actually disagree on real hardware.

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
