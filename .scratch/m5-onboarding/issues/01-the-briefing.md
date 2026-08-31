# 01: The Briefing — the text, and nothing captured before it

**What to build:** ADR-0007's blunt one-time legal briefing, which M1 has been standing in for with a one-line `acknowledge` command since the first commit.

**Blocked by:** nothing.

Status: done — the text exists and is shown; counsel review is named as owed

- [x] The Briefing text itself. **This is the deliverable** — the modal is packaging. It must say, plainly: recording without all-party consent is a crime in many jurisdictions; the product stores voiceprints of people who never agreed to that (ADR-0008); Auto-Record is **on** unless turned off (ADR-0023); and copies of the History folder carry biometric data (ADR-0035's own stated consequence)
- [x] **Nothing softened for conversion.** Every clause above exists because an ADR accepted a real cost eyes-open, and an acknowledgment button under reassuring text is a dark pattern rather than a consent surface
- [x] In the CLI, the whole text prints and then a y/N prompt asks — **but only when someone is actually at a terminal.** A script or test invoking `acknowledge` has already made the deliberate choice a prompt exists to elicit, and blocking on a pipe that will never answer would hang instead of asking. The Client's modal is ticket 02's
- [x] **Nothing is captured before it** (ADR-0023). The gate exists and is tested; this ticket must not weaken it, and should add the assertion that acknowledgment cannot be granted by a Client that never displayed the text
- [x] The tray's `NotPermitted` phase already exists for exactly this state and should be what an unacknowledged install shows
- [x] Readable again afterwards — an Operator who wants to re-read what they agreed to should not have to reinstall
- [x] Both, and the Chinese one is explicitly **a translation of a general notice rather than a summary of Chinese law** — PIPL treats biometric data as sensitive personal information and generally requires separate consent, which is not what the English text describes. `AWAITING_COUNSEL` ships with every version and says the notice has not been reviewed by a lawyer. **Counsel review remains owed and is the Operator's to commission**; the PRD makes it mandatory before v1
- [x] True of the CLI today: an unacknowledged install prints the whole Briefing before it will accept an acknowledgment. **The Client-side ordering is ticket 02's**, since there is no onboarding flow yet for it to be first in
