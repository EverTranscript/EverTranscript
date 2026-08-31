# 06: Packaging and signing

**What to build:** ADR-0016 as amended by ADR-0025: Developer ID signed and notarized on macOS, a signed installer on Windows. Direct download, no app store.

**Blocked by:** 05.

Status: done — both artifacts built in CI; only the certificates are outstanding

- [x] Built and verified: `EverTranscript.app` contains `evertranscript` and `evertranscript-summarizer` in `Contents/Resources`, and **the bundled Core runs from inside it** (`evertranscript 0.1.0`). The identifier-agreement trap is documented where whoever signs will read it, because its failure presents as the Core "not starting" with nothing in any log
- [x] The macOS entitlements the product actually needs and **nothing else** — the permission-set audit already forbids ScreenCaptureKit, Contacts, MapKit and CoreLocation, and the bundle is where that becomes visible to an evaluator (story 46)
- [x] Scripted, gated on `NOTARY_PROFILE`, and **never run** — there is no key here. The script says it skipped rather than pretending
- [x] `EverTranscript Setup 0.1.0.exe`, 94 MB, built on `windows-latest` by `.github/workflows/package.yml`. **It has to be built on Windows and that is a real limit, not a missing flag**: `cargo xwin check` type-checks the workspace from a Mac, but *linking* pulls in whisper.cpp's and llama.cpp's static libraries and fails on `ggml-blas`
- [x] **The credentials do not live here and should not.** Developer ID certificates, an App Store Connect key, and a Windows code-signing certificate are the Operator's. Build the pipeline, name the human steps, and say plainly that an unsigned local build is the expected result here rather than a failure to paper over
- [x] One command, a frozen lockfile, and no manual steps. **Not bit-reproducible** — Rust release builds embed paths — which is a different and stronger claim than this criterion makes
