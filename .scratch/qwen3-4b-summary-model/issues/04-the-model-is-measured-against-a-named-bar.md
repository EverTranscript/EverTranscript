# 04: The model is measured against a named bar

**What to build:** The evidence that closes M4's "choose the real default by measurement"
criterion, as a standing test rather than an anecdote. Its subject is the registered
Summary model, and it re-runs the case DECISIONS Q45 measured on the incumbent.

The incumbent's recorded numbers, for comparison: **0 of 1** plain commitments found,
**fabricated** `Said at: 14:00` and `When: Monday` for a transcript containing no times,
and **3 of 3** transcript lines reproduced verbatim.

The three axes are not equal. Missing an action item leaves the record incomplete, which
the product already admits to. Inventing a timestamp puts a false claim *into* a record
ADR-0009 makes permanent — so that one is a gate, not a score.

**Blocked by:** 03.

**Status:** ready-for-agent

- [ ] A model-gated quality test whose subject is the registered Summary model, separate from the platform test, which keeps reporting rather than asserting
- [ ] Zero fabricated timestamps is asserted as a gate
- [ ] Action items found and verbatim echo are required to improve on the incumbent's recorded numbers
- [ ] The generation timeout is re-derived from an observed per-chunk time on this model, replacing a bound whose comment says it was sized for a small model
- [ ] The measured numbers are journaled, including if they disappoint — a bigger model that still fabricates is a finding, not a failed ticket
- [ ] 03 does not merge unless this passes
- [ ] The local gate is green
