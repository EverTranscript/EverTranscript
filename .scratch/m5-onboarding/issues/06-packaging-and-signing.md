# 06: Packaging and signing

**What to build:** ADR-0016 as amended by ADR-0025: Developer ID signed and notarized on macOS, a signed installer on Windows. Direct download, no app store.

**Blocked by:** 05.

Status: not started

- [ ] A macOS app bundle containing the Client, the Core, and the Summary sidecar — three binaries that must be signed and notarized together, and whose codesigning identifiers must agree or the OS will refuse the child processes
- [ ] The macOS entitlements the product actually needs and **nothing else** — the permission-set audit already forbids ScreenCaptureKit, Contacts, MapKit and CoreLocation, and the bundle is where that becomes visible to an evaluator (story 46)
- [ ] Notarization, and the staple. Notarization passes on merits; it is the *Mac App Store human review* of system-audio capture that ADR-0016 rejected, not this
- [ ] A signed Windows installer carrying the same three binaries
- [ ] **The credentials do not live here and should not.** Developer ID certificates, an App Store Connect key, and a Windows code-signing certificate are the Operator's. Build the pipeline, name the human steps, and say plainly that an unsigned local build is the expected result here rather than a failure to paper over
- [ ] The build is reproducible enough to be worth signing: a documented command, pinned toolchains, no manual steps between `checkout` and `artifact`
