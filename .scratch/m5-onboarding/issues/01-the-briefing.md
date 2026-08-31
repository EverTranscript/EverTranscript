# 01: The Briefing — the text, and nothing captured before it

**What to build:** ADR-0007's blunt one-time legal briefing, which M1 has been standing in for with a one-line `acknowledge` command since the first commit.

**Blocked by:** nothing.

Status: not started

- [ ] The Briefing text itself. **This is the deliverable** — the modal is packaging. It must say, plainly: recording without all-party consent is a crime in many jurisdictions; the product stores voiceprints of people who never agreed to that (ADR-0008); Auto-Record is **on** unless turned off (ADR-0023); and copies of the History folder carry biometric data (ADR-0035's own stated consequence)
- [ ] **Nothing softened for conversion.** Every clause above exists because an ADR accepted a real cost eyes-open, and an acknowledgment button under reassuring text is a dark pattern rather than a consent surface
- [ ] Ends in an explicit acknowledgment — a deliberate act, not a dismissed dialog. Closing the window is not acceptance
- [ ] **Nothing is captured before it** (ADR-0023). The gate exists and is tested; this ticket must not weaken it, and should add the assertion that acknowledgment cannot be granted by a Client that never displayed the text
- [ ] The tray's `NotPermitted` phase already exists for exactly this state and should be what an unacknowledged install shows
- [ ] Readable again afterwards — an Operator who wants to re-read what they agreed to should not have to reinstall
- [ ] English and Simplified Chinese. **The Briefing's legal copy is per-jurisdiction counsel work, not translation** (PRD), and the Chinese version must be marked as awaiting that rather than presented as equivalent
- [ ] It is the first thing a new install shows, before anything else in onboarding
