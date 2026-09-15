# 01: The harness measures DER and EER, and reproduces the number M3 recorded

**Parent:** `docs/prd.md` (Speakers & diarization, stories 33b-33i) and ADR-0037.

**What to build:** The measurement everything else in this effort is judged by, committed
this time. M3 owed a Diarization Error Rate, produced one, and deleted the scratch that
produced it — so the 49.7% and the whole embedding bake-off behind ADR-0037 are currently
unreproducible claims in a markdown file.

It takes audio plus a reference and reports two numbers: **DER** on AMI's test set, scored
against BUT's `only_words` reference with no collar and overlapped speech included, which is
the protocol pyannote publishes under; and **cross-meeting EER**, built from per-meeting
voiceprints scored against the other meetings in the same series, where the other voices are
the same colleagues in the same room.

It drives the **shipped** pipeline, not a copy. The close-out's attribution of the gap was
trustworthy because a copy instrumented for analysis produced the same turns on all sixteen
meetings; that property is worth keeping deliberately rather than by luck. An oracle mode
that hands every embedded window its true speaker is what separates turn-placement error
from clustering error, and it is how the next ticket is judged.

The corpus is not committed: AMI Mix-Headset is CC BY 4.0 and BUT's references are
Apache-2.0, both fetched on demand into a cache the test skips loudly without. Nothing here
may run in the default `cargo test` path.

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [ ] DER on AMI test reproduces the recorded 49.7% on the unchanged pipeline, within noise, with the missed/false-alarm/confusion split
- [ ] The oracle mode reproduces the recorded 32.6% floor
- [ ] Cross-meeting EER reproduces the recorded 10.3% for the shipped embedding, and the share of different-colleague pairs above the match floor
- [ ] Per-meeting output, so a single bad meeting is visible rather than averaged away
- [ ] Wall-clock per meeting is reported, since a ceiling is one of the things being fixed
- [ ] Skips loudly without the corpus; never fetches inside the default suite; the zero-network guarantee is untouched
- [ ] Runs on both platforms (ADR-0025)
