# 05: In-app updates, and the switch that turns them off

**What to build:** ADR-0016 as amended: electron-updater, cross-platform, covering the bundled Core.

**Blocked by:** nothing.

Status: the check and its switch exist; electron-updater does not

- [ ] **Not wired.** What exists is the Core-side check — the host, the version comparison, and the switch — which is the half that touches the guarantee. Installing an update is the other half and needs the signed artifacts ticket 06 produces, so wiring electron-updater against unsigned local builds would prove nothing. Original criterion: — a Client that updated itself and left an old Core behind is a protocol-skew bug waiting for its first user
- [x] **Disableable in Settings** (ADR-0034), and the setting is honoured before any check happens rather than after
- [x] **With updates off and models downloaded, the product makes zero network calls.** The guarantee test already asserts this and must keep passing with the updater present — which is the whole reason the switch exists
- [x] The update check sends nothing about the machine: no identifiers, no version telemetry beyond what a version check inherently is, no meeting data. It is entry one of three (ADR-0034) and should be inspectable in one screenful
- [x] A failed update check is silent-ish: it is not an error the Operator must dismiss, and it never blocks recording
- [x] Both platforms
