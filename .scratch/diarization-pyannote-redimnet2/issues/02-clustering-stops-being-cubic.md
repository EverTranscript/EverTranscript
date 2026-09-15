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

**Status:** ready-for-agent

- [ ] DER on AMI test is unchanged within noise from 01's baseline
- [ ] Clustering time is roughly linear in meeting length across the corpus, and a two-hour meeting is demonstrated in minutes
- [ ] Results stay independent of input order, the property the existing closest-pair-first pass has and a blocked one can lose
- [ ] The existing agglomeration tests still pass, or their replacements assert the same properties
