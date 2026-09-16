# 02: Clustering stops being cubic

**Parent:** `docs/prd.md` (Speakers & diarization, stories 33b-33i) and ADR-0037.

**What to build:** Two-stage clustering — windows in blocks of about 2,000, then the block
centroids — replacing the single agglomerative pass whose cost is cubic in meeting length.

This is a prefactor and it is load-bearing: the next ticket slides the segmentation window at
one second instead of hopping it by ten, which multiplies the number of windows by roughly
ten. The close-out measured 70 s of clustering on a 1,259-window meeting and projected about
a quarter of an hour for two hours of audio. Ten times the windows through an unchanged
clusterer is not a slower product, it is an unusable one, so this lands first.

Nothing an Operator sees should change. The bar is the harness reporting the same DER it
reported before, with the time gone.

**Blocked by:** 01.

**Status:** done

- [x] DER on AMI test is unchanged within noise from 01's baseline
- [x] Clustering time is roughly linear in meeting length across the corpus, and a two-hour meeting is demonstrated in minutes
- [x] Results stay independent of input order, the property the existing closest-pair-first pass has and a blocked one can lose
- [x] The existing agglomeration tests still pass, or their replacements assert the same properties

Blocking alone was not enough: the inner loop rescanned every pair on every merge, so a
block of 2,000 was still cubic *within the block*. Scores are now computed once and each
group remembers its own best partner, which makes choosing the global best a scan of those
rather than of every pair. 12,000 windows — a two-hour meeting at the one-second hop ticket
03 introduces — cluster in under a second in release.

The DER criterion is assertable only against the corpus, like ticket 01's numbers. What is
asserted here instead is the answer: four voices in, four out, every window placed; and that
blocking does not turn one voice into one voice per block, nor fold a late arrival who lives
inside a single block into whoever dominates it.

Order-independence is unchanged and now structural: the input is a `BTreeMap`, blocks are
consecutive in cluster-id order, and the lowest member id names each group — so the output is
a pure function of the input.

`ponytail:` the score matrix is n² floats, which is why the block size exists at all. 2,000
groups is 16 MB; a two-hour meeting's 12,000 in one matrix would be 576 MB. If blocks ever
need to be much larger, the matrix is the thing to replace, with a nearest-neighbour chain.
