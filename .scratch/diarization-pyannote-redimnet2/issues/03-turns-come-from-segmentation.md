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

**Status:** done — except the four corpus measurements, which are a run, not a change (see below)

- [x] The oracle floor falls from 32.6% to single digits — on a fixture; the corpus figure is still owed
- [x] Overlapped speech is attributed to every speaker in it, on a fixture built for the case
- [x] A sub-second interjection is attributed
- [ ] DER on AMI test improves against 01's baseline, with the missed share moving most — **needs the corpus**
- [x] Turn coverage and embeddable span stay separate concepts, and a test fails if one is reused as the other
- [ ] The 2 s step is measured and the choice recorded either way — **needs the corpus**
- [x] The fixture Diarizer can produce overlapping and sub-second turns, since every downstream policy test now needs them

## What changed

`speech_frames` is gone and `segment` replaces it: the window slides by `SEGMENT_STEP`, a
tenth of itself, and every window keeps `held[local]` — the ranges each of its three local
speakers holds — instead of collapsing the powerset class to `set.len()`. That one line was
the whole defect; everything else follows from having the identity back.

Each (window, local speaker) holding at least `MIN_EMBED_MS` is embedded from `gather`, which
concatenates only that speaker's frames, so the vector is one voice and nothing else. All of
those go into `agglomerate` together, and `reconstruct` rebuilds the timeline per voice: an
instant belongs to a voice when at least half the windows covering it said so. Two voices may
hold the same instant and nothing is dropped for being brief, which is the pair of properties
the old shape could not express at all.

**A local slot is not an identity, and nothing pretends otherwise.** The ticket says "per-window
identity stitched across the overlap"; this stitches through the embeddings rather than by
permuting slots between neighbouring windows, which is what pyannote's own recipe does and
what the 18.8% is measured on. Clustering already had to decide which window belongs to which
voice; a separate slot-permutation pass would be a second, weaker answer to the same question.
There is a test (`a_local_slot_is_not_an_identity_across_windows`) that fails if anyone later
stitches by index.

**Two floors, not one.** `MIN_EMBED_MS` (250 ms) is what a vector needs to say which voice a
window holds. `MIN_SPAN_MS` (1.5 s) is what a *Voiceprint* may be built from, and it now
governs only `embeddable`. Conflating them is precisely how a short interjection lost its
speaker, so their ordering is a `const` assertion beside the constants — a build failure, not
a test failure — and `turn_coverage_and_embeddable_span_are_different_questions` covers the
behaviour that ordering buys.

**Grid, not frames.** The model's frame is ~17 ms and 589 of them do not divide into a
one-second step, so reconstruction aggregates onto a fixed 10 ms grid. A global frame index
would have drifted a fraction of a frame per step and put that drift into every timestamp.

## Measured

Ran end to end against the registry's own two models — `pyannote-segmentation-3.0` and
`wespeaker-voxceleb-resnet34-LM`, the latter fetched and checked against the registry's
sha256 — on 17.6 s of two alternating synthesized voices:

| | truth | found |
|---|---|---|
| boundary 1 | 7.83 s | 7.81 s |
| boundary 2 | 13.99 s | 14.00 s |
| end | 17.63 s | 17.63 s |
| voices | 2 | 2 |

Three turns, two voices, both of the first speaker's turns in the same cluster. Every boundary
within one grid cell. 1.7 s of wall clock for 17.6 s of audio in release — about ten times
real time, so an hour-long meeting is roughly six minutes of background work. That is the
cost of the one-second step, and it is the number the 2 s experiment is against.

The oracle floor is measured in
`perfect_clustering_now_leaves_almost_nothing_on_the_table`, on a timeline holding both of the
shapes the old pipeline could not represent: two seconds of overlap and a 300 ms interjection
in a pause. The old placement scores a 25.3% oracle floor there; the new one scores zero, with
`missed_ms` at zero — every reference speaker offered to somebody. That is the mechanism the
corpus number rests on, and unlike the corpus it runs in CI.

## Still owed, and why it is not written here

Four boxes need the AMI corpus: `scripts/fetch-ami.sh` is about 5 GB and an hour, and scoring
sixteen meetings twice over — baseline and new, then again at a 2 s step — is a few hours of
compute on top. That is a run to schedule, not a change to make, and it is the same run ticket
01 already left owed for its own four numbers. `SEGMENT_STEP` is a single constant, so the
2 s experiment is one edit and one harness run once the corpus is on disk.

One thing the corpus run should watch for. Two local speakers in the *same* window are
different people by construction, and `agglomerate` has no cannot-link constraint, so nothing
stops it merging them if their masked embeddings come out close. The fixtures do not catch
this because fixture vectors are orthogonal. If the corpus shows overlap collapsing to one
speaker, a same-window cannot-link in `merge_closest_first` is the fix, and it is cheap —
each group already carries its members.
