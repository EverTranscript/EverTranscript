# 04: Tray and Electron integration

**What to build:** The menu-bar item shows the mark instead of the text `●`/`○`: `TrayView.indicator` becomes a `TrayIndicator` enum (Ready / Recording / Busy / Attention), and `tray/macos.rs` embeds the four template TIFFs and sets a template `NSImage` per state. The Electron Client sets its window icon (Windows/Linux) and Dock icon (macOS, unpackaged) from `clients/electron/resources/`. Both CI legs stay green: the embedded bytes live only inside the macOS-only module.

**Blocked by:** 03.

**Status:** resolved — the glyph has been seen: the e alone with a hollow dot (Attention, on a machine with no model), the e with a solid dot while a real Meeting recorded (2026-08-28, screencapture after the Operator granted Screen Recording to sshd-keygen-wrapper)
