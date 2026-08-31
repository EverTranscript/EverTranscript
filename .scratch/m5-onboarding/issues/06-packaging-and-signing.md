# 06: Packaging and signing

**What to build:** ADR-0016 as amended by ADR-0025: Developer ID signed and notarized on macOS, a signed installer on Windows. Direct download, no app store.

**Blocked by:** 05.

Status: pipeline built and run; signing needs the Operator's certificates

- [x] `packaging/build.sh` assembles all three and was run: real binaries, real checksums in `SHA256SUMS`. **Not yet a `.app` bundle** — the three artifacts and the built renderer are staged side by side, and wrapping them in an Electron bundle is the remaining step. The identifier-agreement trap is documented where whoever signs will read it, because its failure presents as the Core "not starting" with nothing in any log
- [x] The macOS entitlements the product actually needs and **nothing else** — the permission-set audit already forbids ScreenCaptureKit, Contacts, MapKit and CoreLocation, and the bundle is where that becomes visible to an evaluator (story 46)
- [x] Scripted, gated on `NOTARY_PROFILE`, and **never run** — there is no key here. The script says it skipped rather than pretending
- [ ] **Not built.** The build script handles the `.exe` suffix and the manifest generator expects the artifact, but nothing produces an NSIS installer yet. Named rather than implied by the surrounding ticks
- [x] **The credentials do not live here and should not.** Developer ID certificates, an App Store Connect key, and a Windows code-signing certificate are the Operator's. Build the pipeline, name the human steps, and say plainly that an unsigned local build is the expected result here rather than a failure to paper over
- [x] One command, a frozen lockfile, and no manual steps. **Not bit-reproducible** — Rust release builds embed paths — which is a different and stronger claim than this criterion makes
