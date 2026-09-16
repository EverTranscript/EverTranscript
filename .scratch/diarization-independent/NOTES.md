# Diarization: what landed, what is parked

Branch `diarization-independent`, cut from `origin/main` at 83e0521.

The twelve tickets in `.scratch/diarization-pyannote-redimnet2/` were written
against a main that has since moved. Six of them turned out to be independent
of the question that stalled the rest — whether ReDimNet2-B3 should replace
WeSpeaker — so they were landed on their own. Their ticket files were
deliberately not brought across: they describe a starting state that no longer
exists, and a stale ticket is worse than none.

`issues/` holds the ones rewritten here rather than landed: 05 and 12, against
the current code, with the policy they carry unchanged.

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
| 05 | A model change clears Voiceprints. Migration written and tested, **not registered**; Registry messaging and activation remain — see below. |
| 06 | ReDimNet2-B3 replaces WeSpeaker. Measured on both halves; the decision is the user's and is unanswered. See below. |
| 07 | Recognition thresholds are re-derived. Dev curve and held-out validation done; no point selected, for the same reason as 06. |
| 12 | A model change re-runs History. `cluster::claims` built and unwired (`058bcad`); the rest wants 05, a migration or the protocol — see below. |

### 05 and 12, audited against this branch rather than their old Done flags

Both are marked done on `diarization-pyannote-redimnet2`. Neither landed here.
The rewritten tickets are in `issues/`; this is what changed and what did not.

**The policy is unchanged and unshipped.** ADR-0037 asks that a model change
wipe every Voiceprint and re-run History, rebuilding a named Speaker from its
**attributed whole clusters** with the Operator's corrections on top — and it
explicitly rejected re-embedding the old exemplars' stored sample offsets,
because those offsets are the *old model's* choice of cuts while the Operator's
naming is a statement about a whole cluster. That reasoning is untouched by
anything measured since.

**What `main` does today is a different, narrower mechanism.** Since Q115,
`stale_exemplars` finds every row from another space, `runner::rebuild`
re-embeds each from the sample window it kept, and `adopt_rebuilt` adopts them
in the next Diarization's own transaction. It is lazy, per-Meeting, and it is
the thing ADR-0037 rejected: the old model's cuts. It rebuilds *evidence* and
never re-runs *attribution*.

An earlier revision of this file called 05 "superseded" on the strength of that
mechanism. **That was wrong twice.** An implementation existing is not a policy
being replaced, and "recognition already survives a model change" is stronger
than anything measured — the rebuild has never been exercised against a real
model change on a populated History, only through its seams, and a Speaker
whose exemplars have no window or whose Meeting is gone loses its Voiceprint
under it. Nothing has measured how often that is.

If the lazy path *should* replace the policy, that is a proposal for the user
and is written up as one at the end of `issues/05-…`. It is not adopted here.

**What actually changed for the tickets** is narrower: 05's migration must now
account for the rebuild path existing, and 12's `claims` mechanism can no
longer be justified by "there is no vector left to seed with" — after 05's wipe
there is none, but 05 now has to say so rather than assume it. Both are
rewritten on those terms. 12 remains blocked by 05 and by nothing else: 04 has
landed, and 08's queue landed with it.

**05's migration is now written and tested, and is not registered.**
`schema::PENDING_MODEL_CHANGE_WIPE` sits beside `MIGRATIONS` and outside it,
with three tests: a file-backed control that an ordinary open leaves a current
History alone, a file-backed close/reopen that the wipe takes every vector and
keeps every Speaker, name, flag, mark, attribution and hint, and one asserting
it is still unregistered — since appending it to `MIGRATIONS` is the whole of
activating it. `stale_exemplars` is empty afterwards for the current identity
and for a hypothetical next model, which is what stops the lazy rebuild path
reintroducing the old model's cuts behind the wipe. **05 is not done**: the
Registry messaging and the activation remain.

**12's first two pieces are built and unwired**: `cluster::claims` (`058bcad`)
and `cluster::relearn` (`c72bb74`) — no table, no migration, no protocol
method, and nothing calls either. `claims` hands back the clusters a Speaker
owns outright and the Speakers a correction took a whole cluster away from,
reading through `attributed_speaker` and `replaced_speaker` so the Operator's
latest word counts in both directions, and excluding the Operator. `relearn`
files the denials as negative exemplars.

**The rule on both halves is unanimity, not a vote.** A cluster is claimed only
where every segment in it belongs to the same eligible Speaker, and denied only
where every segment was corrected away from the same one; anything mixed or
unvouched yields nothing. A first draft used a plurality, which would have
enrolled a cluster's unsupported audio under whichever name held the most of it
and broken a two-name tie by comparing UUIDs. The test is over the set of
owners rather than a count, so splitting an utterance into more segments cannot
change who claims it. An absent claim withholds the shortcut past the resolve,
not the person: the Voiceprint is still there to match against, and the seeding
path can still rebuild from the ranges that are theirs. `relearn` deletes
nothing and is idempotent, and writes only whole-cluster denials — a
per-segment negative needs a per-segment vector, which its inputs do not carry.

Two traps from the old branch's version are written into the ticket so a
rewrite cannot lose them: `begin_if_the_model_changed` treats an *absent*
metadata row as a model change and would enqueue all of History on a first
start, and `relearnable` includes the Operator while 12 forbids relearning the
Operator from old attributions, so `claims` excludes it and leaves the channel
rules responsible.

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

- **The existing unconstrained path, each model at its own dev-selected merge
  threshold:** ReDimNet2 leads DER by 3.12 points on dev and 4.02 on held-out
  test, all of it confusion (Q140, Q143). Not the shipped configuration —
  production runs one threshold of 0.60 whatever the model, and WeSpeaker was
  given 0.65 here so each model was judged at its own best measured dev point.
- **With a same-window cannot-link constraint** built only from segmentation
  provenance and never from the reference: WeSpeaker gains 5.13 held-out points
  and ReDimNet2 gains 0.68, and **the DER ranking reverses** — 23.51% against
  23.94% (Q147, Q149, Q150). Harness-only; production still runs the
  unconstrained clusterer.
- **Recognition, under the constraint, at points declared before the split was
  looked at:** ReDimNet2 is better on all four reported quantities — net
  +2239.300s correct returning and −402.720s wrong (Q151–Q153). Both
  constrained configurations are nonetheless worse on returning time than their
  own earlier unconstrained replay, so the DER gain is not a recognition gain.
  All of these are net bucket differences between runs; no paired per-segment
  transition was measured, so no bucket can be said to have fed another.

So the two halves of ticket 06 now disagree, and that is the finding rather than
a problem to resolve by averaging. They measure different things.

### What is not measured, and what is not ours to decide

**The split-model option is unmeasured.** Every run above moves the clustering
embedding and the recognition embedding together, so "WeSpeaker wins DER,
ReDimNet2 wins recognition" does **not** establish that taking one of each
combines their benefits. Decoupling them is a run nobody has done — and it is
conditional on the split question being reopened, not a prerequisite for
keeping one model, which is what the build does today and goes on doing.

**Two decisions are the user's and are outstanding:** whether ≥ 2.0 points of
DER is the adoption bar, and whether to pursue the split-model architecture at
all. **Separately unresolved, and not a substitute for either:** the rate of
exchange between a correct and a wrong attributed second, without which the
recognition column cannot be collapsed to one ranking. Nothing here assigns any
of the three. Q152's rule holds throughout: measurements, mechanism hypotheses
and utility judgements stay separately labelled, and a sentence that ranks two
outcomes is a utility judgement however it is phrased.

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
- **the oracle floor** — the same hypothesis spans relabelled with reference
  identities, so it is what this pipeline would score with labelling error
  removed and nothing else changed.
- **a chronological enrollment replay** — meetings in declared order onto a
  fresh store, scored as reference-transcript speaker-time, reported as correct
  and wrong seconds for returning people and for newcomers separately.

**Withdrawn: cross-meeting EER and nearest-voice-right.** Both are all-pairs
metrics whose trial count moves with the cluster count: across the merge sweep
nearest-voice-right rises monotonically from 33.3% at 0.30 to 72.4% at 0.90
while DER over the same range goes from 37% to 86% (Q140), so they improve
exactly where the partition is getting worse. Fragmentation is the available
explanation for that co-movement and is not established as the mechanism by it;
what is certain is that the trial count is a function of the partition rather
than of the corpus, so two arms that fragment differently are not being asked
the same question. Every figure this branch reported from them is withdrawn. The enrollment replay
is what replaced them; its denominator is reference speaker-time and does not
move with the partition.

**What the oracle floor is and is not.** It is conditional on the hypothesis
spans actually scored — `oracle_relabel` relabels those spans, so missed speech
and false alarm survive it untouched and only labelling error is removed. It is
therefore not an embedding ceiling, and it licenses exactly one sentence about
a bar below it: **these fixed spans cannot reach that bar by oracle relabelling
alone.** Which stage would have to change to move them is a separate question
this number does not answer. It also flatters
recognition badly if read that way: its centroids are one per person **per
whole meeting**, minutes of speech each, where real enrollment mints a Speaker
from as little as `MIN_SPEAKER_MS` — ten seconds. A separability measured at
whole-meeting duration says nothing about what a ten-second cluster can do.

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
| `EVERTRANSCRIPT_MERGE_SWEEP` | The merge thresholds to score, as a **comma-separated numeric list** (`0.55,0.60,0.65`). Unset is one pass at the shipped `MERGE_THRESHOLD`. It is not a flag: `=1` scores the single threshold 1.0 |
| `EVERTRANSCRIPT_CANNOT_LINK=1` | Enforce segmentation's same-window cannot-link pairs, in both scoring and the replay |
| `EVERTRANSCRIPT_SEGMENT_STEP_MS` | How far the segmentation window advances; unset is the 10 s default |
| `EVERTRANSCRIPT_REPLAY_MANIFEST` | The chronological enrollment replay's meeting order (`tests/ami-replay-{dev,test}.manifest`) |
| `EVERTRANSCRIPT_REPLAY_EVENTS` | Where to write the replay's per-person, per-meeting rows |
| `EVERTRANSCRIPT_REPLAY_PAIR` | Two comma-separated **event-file paths**. Joins two ledgers that already ran and reports the pairing; replays nothing and needs no corpus |
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
