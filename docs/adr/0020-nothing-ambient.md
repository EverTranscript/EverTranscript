# Nothing Ambient: the product is inert until the Operator acts

> **Amended by ADR-0023:** the "no detection exists" clause below is reversed — Meeting Detection (application + microphone state, never content) exists, and under Auto-Record it starts recording by itself; the "'I forgot to record' is an accepted risk" consequence is likewise obsolete. Everything else stands: no calendar, no screen pixels, no filesystem indexing, no contacts, no content observation.

The input-side twin of the Closed Boundary (ADR-0001), fully strict: the product processes no input it wasn't explicitly handed. No calendar (rejected outright, even read-only EventKit), no screen pixels, no filesystem indexing, no contacts — and no ambient STATE observation either: the app does not watch for meeting apps or microphone activity. Recording starts only because the Operator hits Record. **This amends ADR-0007: the "detection may prompt" allowance is removed; no detection exists.**

Explicit one-shot acts remain open by definition — e.g., a future explicit audio-file import — because anything the Operator hands over per-act is not ambient.

## Consequences

- "I forgot to record" is an accepted, owned risk; the mitigation is making Record maximally frictionless (menu-bar item, global hotkey), not detection.
- Meeting titles are hand-authored or suggested post-meeting from the Transcript itself (processing captured content is not an input question).
- Screen awareness (ambient screen context) is permanently out.
- The marketing sentence this buys: "It hears your meetings when you press Record — and touches nothing else on your machine, ever."
