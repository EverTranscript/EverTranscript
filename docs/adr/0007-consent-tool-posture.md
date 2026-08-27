# Recording consent: tool posture with hard guarantees

> **Amended by ADR-0020:** the "meeting detection may prompt" allowance below was removed — no detection exists; recording is fully manual.

Consent for recording Participants is the Operator's legal obligation; the product is a tool (like a microphone or OBS) and does not enforce, attest, or disclose on the Operator's behalf. It ships three legible things instead: (1) a blunt one-time first-run legal briefing ending in an explicit acknowledgment — recording without all-party consent is a crime in many jurisdictions; (2) a by-construction guarantee that recording never starts without an explicit Operator action — meeting detection may prompt, never start; (3) an always-visible Operator-facing recording indicator.

## Considered options

Per-meeting attestation gates (click-through theater that protects the vendor, not Participants), active disclosure features (automatic outward behavior with no channel to speak through), and jurisdiction-aware enforcement (counterparty jurisdiction is unknowable; it manufactures a false guarantee) were all rejected.
