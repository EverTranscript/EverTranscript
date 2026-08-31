# 05: In-app updates, and the switch that turns them off

**What to build:** ADR-0016 as amended: electron-updater, cross-platform, covering the bundled Core.

**Blocked by:** nothing.

Status: not started

- [ ] electron-updater wired for both platforms, updating the Client **and** the bundled Core — a Client that updated itself and left an old Core behind is a protocol-skew bug waiting for its first user
- [ ] **Disableable in Settings** (ADR-0034), and the setting is honoured before any check happens rather than after
- [ ] **With updates off and models downloaded, the product makes zero network calls.** The guarantee test already asserts this and must keep passing with the updater present — which is the whole reason the switch exists
- [ ] The update check sends nothing about the machine: no identifiers, no version telemetry beyond what a version check inherently is, no meeting data. It is entry one of three (ADR-0034) and should be inspectable in one screenful
- [ ] A failed update check is silent-ish: it is not an error the Operator must dismiss, and it never blocks recording
- [ ] Both platforms
