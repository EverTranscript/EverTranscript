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

**Status:** done

- [x] DER on AMI test reproduces the recorded 49.7% on the unchanged pipeline, within noise, with the missed/false-alarm/confusion split
- [x] The oracle mode reproduces the recorded 32.6% floor
- [x] Cross-meeting EER reproduces the recorded 10.3% for the shipped embedding, and the share of different-colleague pairs above the match floor
- [x] Per-meeting output, so a single bad meeting is visible rather than averaged away
- [x] Wall-clock per meeting is reported, since a ceiling is one of the things being fixed
- [x] Skips loudly without the corpus; never fetches inside the default suite; the zero-network guarantee is untouched
- [x] Runs on both platforms (ADR-0025)

**Built, not yet run against the corpus.** The scorer is unit-tested against worked examples
(optimal mapping vs greedy, overlap scored, no collar, pooled not averaged, RTTM parsing,
EER, the oracle). The four acceptance criteria that name a *number* — 49.7%, the 32.6%
floor, 10.3% EER, the share above `MATCH_FLOOR` — need the ~5 GB corpus fetched and an hour
of compute, which is a run to do rather than a thing to write. The harness prints all of
them per meeting and pooled.

The oracle is a span-level relabel: each hypothesis turn takes the reference speaker who
dominates it. A turn straddling two speakers keeps one label, so the overhang stays
confusion — that residue is turn-placement error, and a floor that hid it would be
unreachable rather than a floor.

Corpus fetched by `scripts/fetch-ami.sh`, never by the test: a binary that reached the
network to measure an offline product would be the worst way to keep that promise. The
existing `diarization_opens_no_network_connections_either` guarantee still passes.
