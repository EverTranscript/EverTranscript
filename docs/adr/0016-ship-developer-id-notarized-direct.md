# v1 ships Developer ID signed, notarized, direct — not Mac App Store

> **Amended by ADR-0025:** the same posture extends to Windows — signed installer, direct download plus winget, no Microsoft Store — in v1 itself (simultaneous per ADR-0025's 2026-08-27 staging reversal), and the updater is electron-updater (cross-platform, covers the bundled Core) rather than Sparkle.

Direct download plus a Homebrew cask, Developer ID signed, notarized, Sparkle for updates. Notarization passes on merits; Mac App Store *human* review is the real risk — sandbox fights over system-audio capture and review latency on every update. This also matches how the entire category ships (VoiceInk, Handy, Vibe, Meetily). A MAS SKU is a post-v1 growth question, not a v1 one.
