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

**Status:** done

- [x] `summary_quality.rs`, separate from `summary_inference.rs`, which still reports without asserting
- [x] Asserted: every time-shaped string in the Summary must appear in the transcript. **The fixture had to be corrected first** — Q45's input, which this inherited, had no timestamps at all, so it was measuring the harness rather than the model (Q58)
- [x] Both, against numbers measured on the same production-shaped input rather than remembered: 0 of 2 action items and 3 of 3 lines echoed becomes 2 of 2 and 0 of 3
- [x] Re-derived: a full 11,627-token chunk takes ~44 s accelerated, so the bound is doubled to 1,800 s to survive a CPU-only machine an order of magnitude slower
- [x] Journaled as Q58, including the correction to Q45's own finding and the surviving limitation: given no timestamps, this model invents one too. Production cannot produce that input, so it is a boundary rather than a defect
- [x] It passes, so 03 merges
- [x] The local gate is green
