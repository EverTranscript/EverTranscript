# 04: Clustering, cross-meeting persistence, and conservative matching

**What to build:** Turning embeddings into Speakers — within a Meeting, and across all of them. This is where "every voice resolves to a persistent Speaker" (story 28) is either true or a slogan.

**Blocked by:** 01, 02, 03.

Status: not started

- [ ] Agglomerative clustering on L2-normalized embeddings at the catalog's merge threshold, with the spectral fallback and speaker cap it specifies
- [ ] **Cross-meeting persistence is a bias, not a second system** (catalog M3): seed each Meeting's clusterer with prior Voiceprints as frozen speakers at negative timestamps, so recognition falls out of clustering through one code path. A separate post-hoc matching stage can disagree with the clusterer, and then neither answer is defensible
- [ ] **Matching is conservative and structurally so**: cosine above a floor, **and** a margin over the second-best candidate, **and** mutual-best agreement in both directions (catalog M3). A confident wrong attribution costs more than an unnamed Speaker, because the Operator must notice it before they can correct it
- [ ] Confirmed Voiceprints outrank unconfirmed ones when matching, and unconfirmed ones still match, conservatively (ADR-0008 as amended)
- [ ] Unnamed Speakers get numbered pseudonyms — "Speaker 1", "Speaker 2" — stable within a Meeting and never silently renumbered afterwards, since the number is what the Operator refers to when naming
- [ ] A returning Speaker across two Meetings is recognized, driven through the fixture — this is story 28's whole content and it must be a test, not a demo
- [ ] Bounded exemplar growth: a Speaker seen in two hundred Meetings must not carry two hundred vectors into every subsequent clustering run
- [ ] Both platforms in CI (ADR-0025 as amended)
