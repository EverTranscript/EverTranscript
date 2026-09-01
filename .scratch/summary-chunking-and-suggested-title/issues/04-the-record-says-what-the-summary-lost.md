# 04: The record says what the Summary lost

**What to build:** A Summary built from five of six chunks stops wearing the face of a
complete one. What ticket 03 tolerates, this ticket discloses, end to end: an additive
column following the audio-notes pattern, written in the same UPDATE as the Summary so
the existing Mirror triggers fire; the Mirror renders it beside the Summary; an
additive optional field carries it to Clients (ADR-0028); the Client shows it. The
name avoids "notes" everywhere — the glossary reserves that word for Operator writing.

**Blocked by:** 03 (the loss it discloses is born there).

**Status:** ready-for-agent

- [ ] An additive migration adds the nullable gaps column; existing rows are untouched and old Cores reading the store are unaffected
- [ ] A run that lost chunks records what was lost in the same write as the Summary; a complete run records nothing
- [ ] The Mirror renders the gap note beside the Summary, and a regenerated Mirror carries it
- [ ] The Meeting's protocol response carries the gaps as an additive optional field; regenerated bindings and schemas are committed with the change
- [ ] The Client displays the gap note beside the Summary
- [ ] No user-visible surface or schema name uses the word "notes" for this
- [ ] Tests script the loss at the Backend trait seam and observe store, Mirror, response and Client-facing field at the protocol seam; the local gate is green
