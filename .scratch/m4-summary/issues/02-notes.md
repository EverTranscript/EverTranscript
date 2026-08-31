# 02: Operator Notes — the one mutable thing in the record

**What to build:** ADR-0018's Notes: Operator-authored markdown per Meeting, always editable, in the Mirror, and passed to Summary as steering context.

**Blocked by:** nothing.

Status: not started

- [ ] Notes are a first-class entity in SQLite, one per Meeting, created empty and editable forever
- [ ] **This refines ADR-0009 rather than contradicting it, and the code should make that legible**: the *record* — Transcript, attribution — is immutable; Notes are the Operator's own writing and are not part of the record. A future reader must be able to tell why one is rewritable and the other is not
- [ ] Notes render into the Mirror's existing `## Notes` section, replacing `*None yet.*`
- [ ] Editing Notes marks the Mirror dirty, so the folder follows the database (the same obligation M3's rename had)
- [ ] Operator content lives in the Notes entity, **never** in hand-edits to Mirror files — the Mirror stays a regenerable projection (ADR-0005). A Mirror edited by hand is overwritten, and that has to be true rather than merely documented
- [ ] Notes reach Summary generation as steering context (ADR-0018): what the Operator bothered to write down is the strongest signal of what mattered
- [ ] Protocol additions are additive (ADR-0028); bindings and schemas regenerated and committed
- [ ] Notes can be written during a Meeting as well as after — the pane is not a post-meeting form
