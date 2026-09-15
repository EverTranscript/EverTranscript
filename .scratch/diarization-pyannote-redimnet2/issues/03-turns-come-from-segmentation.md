# 03: Turns come from segmentation, not from the rules about what a Voiceprint is built from

**Parent:** `docs/prd.md` (Speakers & diarization, stories 33b-33i) and ADR-0037.

**What to build:** The 32.6 points that survive perfect clustering. This is the largest single
change in the effort and the only one that needs no migration, because it changes what the
pipeline computes rather than what History stores.

Today segmentation is reduced to a per-frame speaker **count**: the powerset output says
*which* local speakers are talking and the pipeline keeps only *how many*, then recovers
identity from embeddings alone. A frame counts as speech only when exactly one voice holds
it, so overlap longer than the merge gap produces no turn for anybody; and turns are made
only from material long enough to embed, so a short interjection gets no speaker. On AMI that
is 27% of speaker-time overlapped and another 6% single-speaker and too short.

Afterwards: windows slide at **one second**, one embedding per local speaker per window built
from the frames that speaker holds with other speakers' frames masked out, per-window
identity stitched across the overlap rather than discarded, and reconstruction per speaker so
two people may hold the same second and nothing is dropped for being brief. The 1.5-second
minimum and the middle-ten-second clip stay exactly where they belong, choosing what a
Voiceprint is built from.

The step is one tenth of the window because that is what pyannote's published 18.8% uses.
Measure a 2 s step afterwards through the harness and keep it only if DER holds within a
point; it is roughly half the compute.

**Blocked by:** 02.

**Status:** ready-for-agent

- [ ] The oracle floor falls from 32.6% to single digits — this is the ticket's real subject, and it is measurable before any embedding changes
- [ ] Overlapped speech is attributed to every speaker in it, on a fixture built for the case and on the corpus
- [ ] A sub-second interjection is attributed
- [ ] DER on AMI test improves against 01's baseline, with the missed share moving most
- [ ] Turn coverage and embeddable span stay separate concepts, and a test fails if one is reused as the other
- [ ] The 2 s step is measured and the choice recorded either way
- [ ] The fixture Diarizer can produce overlapping and sub-second turns, since every downstream policy test now needs them
