# 02: Operator Notes — the one mutable thing in the record

**What to build:** ADR-0018's Notes: Operator-authored markdown per Meeting, always editable, in the Mirror, and passed to Summary as steering context.

**Blocked by:** nothing.

Status: done

- [x] A column on `meetings` rather than its own table. **Deviation from the letter of ADR-0018**, which says "a first-class entity in SQLite": there is exactly one per Meeting and it is never queried independently of one, so a table would add a join and an orphan case to buy nothing. First-class in the sense the ADR is actually protecting — it is in the record, in the Mirror, and reaches Summary — and not in the sense of having its own primary key
- [x] **This refines ADR-0009 rather than contradicting it, and the code should make that legible**: the *record* — Transcript, attribution — is immutable; Notes are the Operator's own writing and are not part of the record. A future reader must be able to tell why one is rewritable and the other is not
- [x] Notes render into the Mirror's existing `## Notes` section, replacing `*None yet.*`
- [x] Editing Notes marks the Mirror dirty, so the folder follows the database (the same obligation M3's rename had)
- [x] Unchanged and already true: the projection worker rewrites the whole file from the record on every rebuild, so a hand-edit is overwritten by construction rather than by a rule anyone has to remember. Writing Notes marks the Mirror dirty, which is what makes the overwrite happen promptly rather than eventually
- [x] Stored and reachable; **the wiring into the prompt is ticket 03's**, since there is no prompt yet to steer. Named here rather than ticked silently
- [x] Protocol additions are additive (ADR-0028); bindings and schemas regenerated and committed
- [x] Notes can be written during a Meeting as well as after — the pane is not a post-meeting form
