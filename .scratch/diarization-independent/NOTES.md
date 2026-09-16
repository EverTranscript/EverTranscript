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

Full suite after the cherry-picks: 812 passed, 1 failed — `detect::macos::tests::a_real_microphone_hold_is_visible_to_the_detector`, which needs a
TCC grant this Mac lacks and fails identically on `main`.

Two commits on top are hand-resolution fallout, not behaviour: `982b6cd` (fmt)
and `c55ea17` (clippy).

## Parked

| Ticket | Why |
|---|---|
| 03 | Turns come from segmentation. Main landed its own turn-placement implementation while this branch built a different one; reconciling them is its own work, not a merge. |
| 04 | A model has an identity. Wants ticket 06's answer first — there is no identity to record until the model is chosen. |
| 05 | A model change clears Voiceprints. Its migration was dropped during the cherry-pick (queue renumbered 12→11, forgotten-column 13→12) because it is downstream of 04. |
| 06 | ReDimNet2-B3 replaces WeSpeaker. **This is the open question.** See below. |
| 07 | Recognition thresholds are re-derived. Held open: the thresholds cannot be fixed while the embedding under them is unsettled. |
| 12 | A model change re-runs History. Downstream of 04 and 05. |

## The open question, and why the old answer does not settle it

ADR-0037 chose ReDimNet2-B3 over WeSpeaker on a bake-off (Q111) that ran all
three candidates through **one front end**, and it was the wrong one for two of
them. Main's own Q115 already records this: the bake-off "compared three models
through the same wrong front end, which is why WeSpeaker looked so much worse
than the ReDimNets there."

So the bake-off measured our feature extraction, not the models. Ticket 06 also
*replaced* the embedding path, which means the swap cannot be evaluated after
the fact — there is nothing left to compare against.

### The rig

`Embedder` now carries a `Frontend` (Q129):

- `Fbank` — Kaldi fbank computed in-crate, fed as `input_features [B,T,80]`;
  WeSpeaker's contract.
- `Waveform` — raw 16 kHz handed to the graph, which owns its mel;
  ReDimNet2-B3's contract.

`observe` picks the frames **once** — alone-frames where numerous enough, all
frames otherwise — and converts them to sample offsets for the waveform path,
so both models see the same audio (Q130). The front end is stated, not sniffed
from the graph's input names: a stale file on this machine was a ReDimNet2
export under WeSpeaker's filename, and a sniffing loader would have compared
ReDimNet2 with itself and reported it as a win.

The harness reads `EVERTRANSCRIPT_EMBEDDING` (`wespeaker` | `redimnet2`) and
prints which model and file it believes it holds.

### What is compared

Four numbers per split, over AMI test (16 meetings) and dev (18):

- **DER** — what the product scores end to end.
- **cross-meeting EER** — how separable the same colleagues are across rooms.
- **nearest voice right** — did the right colleague win, which is the question
  an Operator actually feels and the one EER does not answer.
- **the oracle ceiling** — one centroid per person per meeting, built from the
  reference. What perfect clustering would hand the matcher, so what it reaches
  is the embedding's own ceiling and the gap below it is our clustering's.

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

## Escalated, still open

`Q135` and `Q145` in the **original** branch's journal
(`diarization-pyannote-redimnet2`) remain escalated and were not carried over.
That journal forked from main's at Q115 — both sides claim Q115–Q124 for
different decisions — so it cannot be merged, only read.
