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

**Blocked by:** 06, and now the in-meeting clustering fragmentation described below. No
ticket covers that work yet.

**Status:** held open — criteria 2-4 met, criterion 1 waits on the fragmentation fix
(DECISIONS Q144)

- [ ] **Cross-meeting EER at or under 1%, with no different-colleague pair above the match floor** — the bar this ticket exists for
- [x] Each threshold is chosen on dev and reported on test, with the dev curve recorded
- [x] The nearest-voice-is-right rate is reported alongside, since EER alone does not say whether the right person wins
- [x] The existing recognition tests are re-expressed against the derived values rather than the literals

---

## What was measured

34 meetings, one process each: AMI test (16) and dev (18), Mix-Headset, BUT `only_words`
references. `scripts/fetch-ami.sh <dir> dev` now fetches the dev split. Both splits' voice
and pre-merge window vectors are cached, so re-deriving a threshold reads the cache and
touches no model — `EVERTRANSCRIPT_ANALYZE_ONLY=1` skips the DER pass, turning a two-hour
re-measure into a twenty-five second analysis. Every curve below is printed by
`cargo test -p evertranscript-core --test diarization_accuracy -- --nocapture`.

### Criterion 1 — missed on the first half, met on the second

| | test | dev | bar |
|---|---|---|---|
| cross-meeting EER | **35.82%** at 0.170 | **35.09%** at 0.161 | ≤ 1% |
| different-colleague pairs above `MATCH_FLOOR` 0.62 | **0.00%** | **0.00%** | none |
| nearest voice is right | 41.0% | 36.5% | — |

The second half holds on both splits: nothing that is not the same person clears 0.62. The
first half is missed by more than an order of magnitude.

### Where the miss lives — the number that changes the conclusion

Rebuild each meeting's voices from the reference instead of from the clusterer — one centroid
per person, every window they actually own — and score the identical pairing:

| | test | dev |
|---|---|---|
| oracle cross-meeting EER | **0.00%** at 0.704 | **5.64%** at 0.416 |
| oracle nearest voice is right | **100.0%** | **90.3%** |
| oracle floor admitting nobody | 0.661, refusing 0.00% | 0.916, refusing 25.93% |

**The bar is reachable and the embedding already reaches it.** ReDimNet2-B3 separates these
speakers essentially perfectly on test and well on dev. The 35% is paid entirely by the
in-meeting clustering: it produces ~45 voices per test meeting and ~41 per dev meeting for
about four real people — **ten to eleven Voiceprints per speaker**. Most cross-meeting trials
are therefore one person's shards being compared with another person's shards, and the EER
measures the fragmentation rather than the model.

That makes this ticket's own instrument the wrong lever. No value of the three constants
fixes a partition; the fix is in the clustering, which is ticket 06's and ADR-0037's
territory.

**Resolved (DECISIONS Q144): this ticket is held open rather than closed against criteria
2-4.** The bar is the reason the ticket exists, and the oracle measurement says it is
reachable rather than aspirational — so the bar stays attached to work that can still meet
it. The three constants are settled and landed regardless (Q142); what remains held is
criterion 1 alone.

### Criterion 2 — each threshold chosen on dev, reported on test

**`MATCH_FLOOR` — 0.62, confirmed, not moved.** Dev's false-accept curve falls steeply to
0.60 then flattens while false-reject keeps climbing, so 0.62 is the cheapest point that buys
the whole guarantee.

| threshold | dev FA | dev FR | test FA | test FR |
|---|---|---|---|---|
| 0.30 | 6.79% | 71.18% | 7.27% | 71.22% |
| 0.40 | 1.36% | 88.65% | 1.50% | 89.09% |
| 0.50 | 0.22% | 97.25% | 0.24% | 96.97% |
| 0.55 | 0.07% | 98.75% | 0.10% | 98.58% |
| **0.60** | **0.03%** | **99.40%** | **0.03%** | **99.29%** |
| 0.65 | 0.02% | 99.52% | 0.02% | 99.52% |
| 0.70 | 0.02% | 99.53% | 0.01% | 99.57% |
| 0.80 | 0.01% | 99.57% | 0.01% | 99.63% |
| 0.90 | 0.01% | 99.76% | 0.01% | 99.86% |

The 99% false-reject column is the honest cost of the guarantee and is itself a symptom of the
fragmentation: a shard of a voice does not resemble a whole voice. Against oracle voices the
zero-false-accept floor is 0.661 on test and 0.916 on dev — a quarter of a cosine apart, which
is two seventy-voice splits disagreeing rather than a threshold. So the floor's real
re-derivation waits for the partition to be fixed, instead of being fitted to it now.

**`MATCH_MARGIN` — 0.08, kept against an apparent dev knee at 0.15.** On the pipeline's
current voices dev shows a textbook knee (wrong winners 14.65% at 0.08 → 3.82% at 0.15, flat
after; test 15.80% → 1.89%). That knee is an artefact: those voices are shards, so the curve
is largely one speaker's fragments competing with each other. Against oracle voices there is
no knee to find — on test every winner is already right, so the margin costs and buys nothing
anywhere from 0.00 to 0.20; on dev the few wrong winners hold gaps above 0.17, beyond any
sane margin. Moving it would have looked like progress and changed nothing.

**`MERGE_THRESHOLD` — 0.60, kept; three curves disagree and none can settle it.**

| merge | dev EER | dev nearest | dev purity | dev splinter | test EER | test nearest | test purity | test splinter |
|---|---|---|---|---|---|---|---|---|
| 0.30 | 35.79% | 29.2% | 58.4% | 2.0x | 40.97% | 38.7% | 68.2% | 2.0x |
| 0.40 | 38.21% | 26.5% | 74.1% | 5.2x | 40.76% | 34.0% | 83.1% | 5.2x |
| 0.50 | 36.86% | 35.2% | 83.3% | 10.8x | 38.26% | 37.8% | 90.8% | 12.5x |
| **0.60** | **34.33%** | **40.9%** | **92.1%** | **17.4x** | **35.93%** | **46.9%** | **94.9%** | **19.5x** |
| 0.70 | 29.95% | 49.0% | 96.6% | 26.6x | 33.18% | 53.2% | 97.1% | 27.4x |
| 0.80 | 25.42% | 57.9% | 98.7% | 40.2x | 28.97% | 61.9% | 98.9% | 39.3x |
| 0.90 | 19.64% | 70.6% | 99.7% | 66.6x | 21.32% | 75.8% | 99.7% | 65.5x |

Within-meeting window pairs put the equal-error point at 0.274 (dev) and 0.228 (test), far
below 0.60. Recognition improves monotonically to 0.90 — the edge of the range, with no
interior optimum, which is what a metric looks like when it tracks a shrinking population
rather than a best value, since an unmerged window is a pure window. Purity and splintering
show the trade the recognition column hides: 0.30 buys 2x splintering at 58–68% purity, 0.90
buys 99.7% purity at sixty-odd groups per person. Choosing between them needs DER at each
candidate, and DER needs the windows' timestamps — the cache holds vectors and labels only.
That measurement is owed, and named as owed rather than guessed at.

### Criterion 3 — nearest-voice-is-right

Reported above beside every EER: 41.0% (test) and 36.5% (dev) as shipped, against 100.0% and
90.3% with one voice per speaker. EER alone would not have shown that the right person loses
six times in ten, nor that the embedding wins ten times in ten once the partition is right.
