# System-audio capture is CoreAudio process taps; the macOS floor rises to 14.4

macOS system audio is captured with CoreAudio process taps routed through a private aggregate device (microphone stays cpal), replacing ADR-0025's ScreenCaptureKit line, and the macOS floor moves from 13 to **14.4** (the tap API lands at 14.2; its TCC permission story completes at 14.4). The 2026-08-27 reverse-engineering sweep found taps unanimous among shipping local notetakers — Granola defaults to taps with an SCK fallback (floor 12), anarlog is taps-only (floor 15), Meetily defaults to taps — and taps shrink the sanctioned permission set to **microphone + system-audio recording**: Screen Recording leaves the sheet entirely, which materially strengthens the "verify by entitlements" trust story. Windows capture (WASAPI loopback) is untouched.

## Considered options

ScreenCaptureKit-only (the ADR-0025 plan: floor 13, but heavier, against unanimous convergence, and it drags the Screen Recording permission into every install) and taps-plus-SCK-fallback (Granola's shape: a second capture backend to build and test in v1 for a shrinking 13.x sliver — the scope trap) were both rejected.

## Consequences

- ADR-0024's Google-Meet-via-browser-window-titles mechanism loses the permission it rode on; ADR-0030 replaces it.
- The Nothing Ambient permission audit's expected set changes to exactly microphone + system-audio recording (Screen Recording appears only under ADR-0030's opt-in precision naming).
- Operators on macOS 13–14.3 are excluded from v1; accepted against a mid-2026 install base.
