# 04: Clustering, cross-meeting persistence, and conservative matching

**What to build:** Turning embeddings into Speakers — within a Meeting, and across all of them. This is where "every voice resolves to a persistent Speaker" (story 28) is either true or a slogan.

**Blocked by:** 01, 02, 03.

Status: done — pending the real embeddings from 03

- [x] Agglomerative merging at the catalog's threshold on L2-normalized embeddings. **The spectral fallback and the 32-speaker cap are not built**: both are properties of segmenting real audio into clusters, which is ticket 03's pipeline, and there is nothing here for them to act on yet. Named as owed rather than quietly dropped
- [x] **Cross-meeting persistence goes through one similarity rule**, which is the property the criterion was protecting. `cluster::resolve` is the only place a threshold is applied, and `cluster::seeds` supplies History's Voiceprints to it. **Deviation, stated rather than hidden:** the catalog's literal mechanism is frozen speakers at negative timestamps *inside* the clusterer, and this seeds at the layer above instead — because ticket 01 deliberately kept History out of the seam so a Diarizer could be tested on one file. The danger the criterion names (a post-hoc matcher disagreeing with the clusterer) is answered by there being exactly one threshold rather than by where it lives; when 03's live clusterer wants to bias its own merging, it calls this same `resolve`
- [x] **Matching is conservative and structurally so**: cosine above a floor, **and** a margin over the second-best candidate, **and** mutual-best agreement in both directions (catalog M3). A confident wrong attribution costs more than an unnamed Speaker, because the Operator must notice it before they can correct it
- [x] Confirmed Voiceprints outrank unconfirmed ones when matching, and unconfirmed ones still match, conservatively (ADR-0008 as amended)
- [x] Unnamed Speakers get numbered pseudonyms — "Speaker 1", "Speaker 2" — stable within a Meeting and never silently renumbered afterwards, since the number is what the Operator refers to when naming
- [x] A returning Speaker across two Meetings is recognized, driven through the fixture — this is story 28's whole content and it must be a test, not a demo
- [x] Bounded at `MAX_EXEMPLARS` (32), most recent kept — which bounds drift as well as cost: a voice from three years and one microphone ago is not better evidence than last week's. The centroid is weighted by voiced duration and **excludes negative exemplars**, since averaging in evidence that a voice is *not* this Speaker would move the centroid toward the person it was recorded to distinguish
- [x] Both platforms in CI (ADR-0025 as amended)
