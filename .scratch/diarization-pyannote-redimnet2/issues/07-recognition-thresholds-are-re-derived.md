# 07: Recognition thresholds are re-derived, and the EER bar is met

**Parent:** `docs/prd.md` (Speakers & diarization, stories 33b-33i) and ADR-0037.

**What to build:** The second bar, and an end to three constants that were chosen once and
never re-justified.

The match floor, the margin, and the merge threshold are currently 0.62, 0.08 and 0.60 —
placeholders ADR-0037 names as placeholders. The close-out showed why this is not
bookkeeping: with the shipped embedding, dev could not choose a threshold at all, its curve
flat from 0.40 to 0.60 while test moved eight points over the same range. A threshold that
dev cannot choose is a fragility, and the product was relying on the margin and the
mutual-best rule to stop a colleague being recognized as someone else.

Derive each on AMI's dev set, report on test, and record the curve rather than the point, so
the next person can see how much the choice is worth.

**Blocked by:** 06.

**Status:** ready-for-agent

- [ ] **Cross-meeting EER at or under 1%, with no different-colleague pair above the match floor** — the bar this ticket exists for
- [ ] Each threshold is chosen on dev and reported on test, with the dev curve recorded
- [ ] The nearest-voice-is-right rate is reported alongside, since EER alone does not say whether the right person wins
- [ ] The existing recognition tests are re-expressed against the derived values rather than the literals
