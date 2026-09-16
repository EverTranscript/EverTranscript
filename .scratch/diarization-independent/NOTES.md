# Diarization: what landed, what is parked

Branch `diarization-independent`, cut from `origin/main` at 83e0521.

The twelve tickets in `.scratch/diarization-pyannote-redimnet2/` were written
against a main that has since moved. Six of them turned out to be independent
of the question that stalled the rest — whether ReDimNet2-B3 should replace
WeSpeaker — so they were landed on their own. Their ticket files were
deliberately not brought across: they describe a starting state that no longer
exists, and a stale ticket is worse than none.

## Landed

| Ticket | Commit | What it does |
|---|---|---|
| 01 | `104428e` | The DER and EER harness, committed rather than deleted with the scratch |
| 02 | `b939de2` | Clustering stops being cubic |
| 08 | `e3721ae` | Diarization runs one persistent queue |
| 09 | `c58d347` | A deleted Voiceprint stays deleted |
| 10 | `af0e290` | Naming joins a Speaker that already has that name |
| 11 | `51121b4` | The Operator is identified by three rules, and there is only ever one |
| 04 | `ba8a491` | A model has an identity — from the registry, stamped by whichever model ran, checked before any match |

Full suite after the cherry-picks: 812 passed, 1 failed — `detect::macos::tests::a_real_microphone_hold_is_visible_to_the_detector`, which needs a
TCC grant this Mac lacks and fails identically on `main`. Still the only
failure after ticket 04.

Two commits on top are hand-resolution fallout, not behaviour: `982b6cd` (fmt)
and `c55ea17` (clippy).

Ticket 04's parked reason turned out to be wrong. "There is no identity to
record until the model is chosen" confuses *which* model wins with *which*
model stamped: the identity is a property of whatever ran, and nothing about
it waits on 06. It also had to land before 06's answer rather than after,
because until it did, every vector the whole A/B produced was labelled
WeSpeaker — `provisional_of` stamped a constant. Nothing was measured wrongly,
since the harness throws its store away each run and the two models are
different widths, but the same code shipping a second model would have handed
WeSpeaker Voiceprints to a ReDimNet2 resolve without a word (DECISIONS Q154).

## Parked

| Ticket | Why |
|---|---|
| 03 | Turns come from segmentation. Main landed its own turn-placement implementation while this branch built a different one; reconciling them is its own work, not a merge. |
| 05 | A model change clears Voiceprints. **Superseded on this branch, not parked** — see below. |
| 06 | ReDimNet2-B3 replaces WeSpeaker. Measured on both halves; the decision is the user's and is unanswered. See below. |
| 07 | Recognition thresholds are re-derived. Dev curve and held-out validation done; no point selected, for the same reason as 06. |
| 12 | A model change re-runs History. Still real work, for a narrower reason than it states, and **not** blocked by 04 or 05 — see below. |

### 05 and 12, audited against this branch rather than their old Done flags

Both are marked done on `diarization-pyannote-redimnet2`. Neither landed here,
and the premise both were built on is no longer true of `main`.

Ticket 05 wipes every Voiceprint and every exemplar on a model change, because
"old and new vectors cannot be compared". Main answers the same problem the
other way and has since Q115: `stale_exemplars` finds every row from another
space, `runner::rebuild` re-embeds each from the sample window it kept, and
`adopt_rebuilt` adopts them in the next Diarization's own transaction. A row
with no window is dropped and a Speaker left with nothing loses its Voiceprint,
name and words intact. **Landing 05 as written would delete the evidence that
path rebuilds from**, so it is superseded rather than pending, and the dropped
migration should stay dropped.

Ticket 12's bulk re-run is built on 05 having run first — its `claims`
mechanism exists because "there is no vector left to seed with". On this branch
there is one, rebuilt lazily as each Meeting is next diarized, so recognition
already survives a model change with no bulk job at all. What a re-run would
still buy is narrower and real: the lazy path rebuilds *evidence* but never
re-runs *attribution*, so a Meeting already diarized keeps the turns and the
speaker assignments the old model gave it, and a change to segmentation or turn
placement cannot be fixed by re-embedding anything. That is the ticket worth
writing, and it depends on 08's queue (landed) rather than on 04 or 05.

Checked while auditing and found already correct: `feed_correction` copies the
mistaken exemplar's own model and version rather than stamping the current
ones, and keeps its sample window, so a corrected exemplar is picked up by
`stale_exemplars` like any other. Q115's "still copies the vector rather than
re-embedding" is a freshness note, not an identity hole.

## The open question, and where it now stands

ADR-0037 chose ReDimNet2-B3 over WeSpeaker on a bake-off (Q111) that ran all
three candidates through **one front end**, and it was the wrong one for two of
them. Main's own Q115 already records this: the bake-off "compared three models
through the same wrong front end, which is why WeSpeaker looked so much worse
than the ReDimNets there."

So the bake-off measured our feature extraction, not the models. Ticket 06 also
*replaced* the embedding path, which means the swap cannot be evaluated after
the fact — there is nothing left to compare against.

### What the rig measured

The full table is in ADR-0037's second amendment; the shape of it is:

- **Unconstrained, as the product ships:** ReDimNet2 leads DER by 3.12 points
  on dev and 4.02 on held-out test, all of it confusion (Q140, Q143).
- **With a same-window cannot-link constraint** built only from segmentation
  provenance and never from the reference: WeSpeaker gains 5.13 held-out points
  and ReDimNet2 gains 0.68, and **the DER ranking reverses** — 23.51% against
  23.94% (Q147, Q149, Q150). Harness-only; production still runs the
  unconstrained clusterer.
- **Recognition, under the constraint, at points declared before the split was
  looked at:** ReDimNet2 is better on all four reported quantities, +2239.300s
  correct returning and −402.720s wrong (Q151–Q153). Both constrained
  configurations are nonetheless *worse* on returning time than their own
  earlier unconstrained replay, so the DER gain is not a recognition gain.

So the two halves of ticket 06 now disagree, and that is the finding rather than
a problem to resolve by averaging. They measure different things.

### What is not measured, and what is not ours to decide

**The split-model option is unmeasured.** Every run above moves the clustering
embedding and the recognition embedding together, so "WeSpeaker wins DER,
ReDimNet2 wins recognition" does **not** establish that taking one of each
combines their benefits. Decoupling them is a run nobody has done.

Two decisions are the user's and are outstanding: whether a 2.0-point DER
improvement is the adoption bar, and what rate of exchange holds between a
correct and a wrong attributed second. Nothing here assigns either. Q152's rule
holds throughout: measurements, mechanism hypotheses and utility judgements stay
separately labelled, and a sentence that ranks two outcomes is a utility
judgement however it is phrased.

### The rig

`Embedder` now carries a `Frontend` (Q130):

- `Fbank` — Kaldi fbank computed in-crate, fed as `input_features [B,T,80]`;
  WeSpeaker's contract.
- `Waveform` — raw 16 kHz handed to the graph, which owns its mel;
  ReDimNet2-B3's contract.

`observe` picks the frames **once** — alone-frames where numerous enough, all
frames otherwise — and converts them to sample offsets for the waveform path,
so both models see the same audio (Q131). The front end is stated, not sniffed
from the graph's input names: a stale file on this machine was a ReDimNet2
export under WeSpeaker's filename, and a sniffing loader would have compared
ReDimNet2 with itself and reported it as a win.

The harness reads `EVERTRANSCRIPT_EMBEDDING` (`wespeaker` | `redimnet2`) and
prints which model and file it believes it holds.

### What is compared

Three things per split, over AMI test (16 meetings) and dev (18):

- **DER** — what the product scores end to end.
- **the oracle ceiling** — one centroid per person per meeting, built from the
  reference. What perfect clustering would hand the matcher, so what it reaches
  is the embedding's own ceiling and the gap below it is our clustering's.
- **a chronological enrollment replay** — meetings in declared order onto a
  fresh store, scored as reference-transcript speaker-time, reported as correct
  and wrong seconds for returning people and for newcomers separately.

**Withdrawn: cross-meeting EER and nearest-voice-right.** Both are all-pairs
metrics whose denominators move with the cluster count, so a model or threshold
that fragments more scores better on them for no better reason than that its
fragments are small and pure — nearest-voice-right runs from 33.3% at merge
threshold 0.30 to 72.4% at 0.90 while DER goes from 37% to 86% (Q140). They
cannot compare two models that fragment differently, and every figure this
branch reported from them is withdrawn. The enrollment replay is what replaced
them; it asks the same question with a denominator that does not move.

The oracle ceiling is the one that matters most here: without it a DER
comparison folds embedding quality and clustering quality into one number and
attributes the result to whichever was moved.

### Reproducing

Models in one directory, WeSpeaker's sha256 checked against the registry
(`3955447b0499dc9e0a4541a895df08b03c69098eba4e56c02b5603e9f7f4fcbb`):

```
diarize-embedding.onnx             WeSpeaker ResNet34-LM   26,535,549
diarize-embedding-redimnet2.onnx   ReDimNet2-B3            18,045,013
diarize-segmentation.onnx          pyannote segmentation    5,986,908
```

```sh
cargo build --release -p evertranscript-core --test diarization_accuracy
EVERTRANSCRIPT_MEASURE_DER=1 \
EVERTRANSCRIPT_AMI_DIR=~/ami \
EVERTRANSCRIPT_MODELS_DIR=<dir> \
EVERTRANSCRIPT_EMBEDDING=wespeaker \
  ./target/release/deps/diarization_accuracy-* --nocapture
```

Nothing here reaches the network. The corpus is fetched ahead of time by a
person with `scripts/fetch-ami.sh` (ADR-0002, Story 33).

The other knobs, all harness-side and all unset in production:

| Variable | What it does |
|---|---|
| `EVERTRANSCRIPT_MERGE_SWEEP=1` | Sweep the merge threshold instead of scoring one |
| `EVERTRANSCRIPT_CANNOT_LINK=1` | Enforce segmentation's same-window cannot-link pairs, in both scoring and the replay |
| `EVERTRANSCRIPT_SEGMENT_STEP_MS` | How far the segmentation window advances; unset is the 10 s default |
| `EVERTRANSCRIPT_REPLAY_MANIFEST` | The chronological enrollment replay's meeting order (`tests/ami-replay-{dev,test}.manifest`) |
| `EVERTRANSCRIPT_REPLAY_EVENTS` | Where to write the replay's per-person, per-meeting rows |
| `EVERTRANSCRIPT_REPLAY_PAIR` | Score one named pair of meetings rather than the whole manifest |
| `EVERTRANSCRIPT_MATCHER_GRID=1` | Replay the whole floor × margin grid |
| `EVERTRANSCRIPT_MATCHER_POINTS` | Replay an explicit `floor:margin` list instead of the grid |

The replay infers each meeting once per model and then replays those same
observations into every configuration's own fresh store, so a grid costs one
inference pass rather than one per cell.

## Escalated, still open

`Q135` and `Q145` in the **original** branch's journal
(`diarization-pyannote-redimnet2`) remain escalated and were not carried over.
That journal forked from main's at Q115 — both sides claim Q115–Q124 for
different decisions — so it cannot be merged, only read.
