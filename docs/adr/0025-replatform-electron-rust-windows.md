# Replatform: Electron UI, Rust core and CLI, macOS + Windows

> **Amended by ADR-0027:** macOS system-audio capture is CoreAudio process taps (floor 14.4), not ScreenCaptureKit; Windows WASAPI loopback is unchanged. The protocol's concrete shape is ADR-0028.

The 2026-07-10 direction, superseding the native Swift/SwiftUI macOS-only platform decision: the UI is Electron, all logic and the CLI are Rust (in the mold of openai/codex — a Rust core with every surface as a client of it), and the product targets macOS and Windows. Reach beat native fit: Windows is most of the meeting-heavy market, and the Rust ecosystem covers every load-bearing piece cross-platform — whisper.cpp via Rust bindings, an ONNX Runtime port of the pyannote-family diarization pipeline, `keyring`-style abstraction over Keychain/Credential Manager, WASAPI loopback where ScreenCaptureKit was.

Staging: v1 ships macOS first; Windows is a fast-follow (v1.x), not a simultaneous launch. The Core is cross-platform from day one — capture, detection, autostart, and credentials sit behind platform traits, and CI compiles and tests both targets — so the follow is four trait implementations plus distribution, not a rewrite. Simultaneous v1 was rejected as the scope trap armed: two capture stacks, two signing pipelines, and two QA matrices before a single user validates the product.

## Consequences

- ADR-0014 is amended: whisper.cpp stays, the thin owned wrapper is Rust instead of Swift, and "runs on Metal" becomes per-platform GPU backends (Metal on macOS; Vulkan/CUDA on Windows).
- Detection (ADR-0024) gains a Windows column: Win32 process enumeration and window titles, which require no permission grant — the macOS Screen-Recording-grant story is the harder half.
- Distribution (ADR-0016) grows a Windows half: signed installer, winget, an autoupdater.
- The test harness is Rust, not Swift Testing; the three seams (AudioSource, LLM Backend endpoint, DetectionSource) carry over unchanged as trait boundaries.
- The "verify by entitlements" trust story is macOS-only; on Windows the equivalent claim must lean on observable behavior (e.g., firewall-verifiable absence of traffic) rather than a sandbox manifest.
