# Diarization: what landed, what is parked

Branch `diarization-independent`, cut from `origin/main` at 83e0521.

The twelve tickets in `.scratch/diarization-pyannote-redimnet2/` — on the tag
`archive/diarization-pyannote-redimnet2` now, the branch is gone — were written
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
| 03 | Turns come from segmentation. **Closed (Q210, corrected by Q211/Q212): main's placement already had it.** Main reached the ticket's substance another way — `masks` keeps which local speaker, each is embedded with the others masked out, overlap yields an Observation each, and `MIN_EMBED_FRAMES` is separate from `MIN_SPAN_MS`. Only the sliding step was missing. The grid reconstruction landed (`9381f57`); quantization and its downstream effects explain the measured delta — 0.06 points of pooled oracle floor and 0.2 on one meeting, confirmed by rescoring the same observations at `GRID_MS = 1`, which reproduces the pre-Step-A build exactly. It is not merely that surviving edges move by up to half a cell: rounding both edges feeds `merge_adjacent`, whose 400 ms gap rule can then decide differently. `[0, 1000)` and `[1404, 2000)` are 404 ms apart and stay separate; rounded to the grid the gap is 400 ms and they join. So there is no general “changes no decision at `step == window`” guarantee to claim — only a small measured difference with a known mechanism. The step itself does not pay on the criterion the plan fixed in advance: **0.48 points** of oracle floor at 1 s against a 2-point bar. `SEGMENT_STEP` stays `SEGMENT_WINDOW`. Two defects found in review were repaired before the numbers were trusted (Q211): `assemble` let one window vote twice for a voice it had split across two local tracks, and both same-window diagnostics inferred an observation's source window geometrically instead of reading the recorded one. Corrected, the same-window merge rate is 0.47% → 0.89% → 1.08%, not the twenty-fold rise first reported. The plan's watch item — same-window cannot-link — was **measured and is not worth enabling at the settings tested** (Q214 as corrected by Q216). Enforcing it drives same-window violations to exactly 0 at every step and makes DER **worse** at every step: +0.43 at 10 s, +1.79 at 2 s, +5.70 at 1 s (23.88% → 29.58%, with IB4003 15.1% → 31.6%). Of the same-window pairs with known reference labels, about a third carry the *same* dominant label — 369/1227 at 10 s, 1913/6086 at 2 s, 3770/12189 at 1 s — so the premise that two local tracks of one window are different people does not hold generally. Those counts are descriptive: they count forbidden pairs whether or not the unconstrained run merged them, so they are not lost merge decisions, and they do not divide against the 91 prevented merges to give a ratio or to explain the DER rise, which the decomposition does not attribute. How many of them the unconstrained run would in fact have merged was not measured. **This is six meetings at a fixed merge threshold of 0.60** and rejects enabling the constraint there; it is not a general verdict and does not override the earlier dev-tuned 0.10 and held-out constrained results. Adoption is the user's. One finding is left open for the user, not closed by this ticket: the slide cuts end-to-end DER by ~6 points (29.85% → 23.88%) through falling confusion rather than placement, at 9.3× the segmentation inference. Why confusion falls is untested. |
| 05 | A model change clears Voiceprints. Migration written and tested, **not registered**. Registry messaging is done — the wipe keeps the model stamp so a named Speaker can say *why* it is empty (Q184, `3d65e16`). The migration-index criterion is closed ahead of registration rather than after it (Q223): all fourteen migrations are named constants and the three upgrade-path tests derive their index from the migration they mean, so an insertion anywhere but the end cannot leave them silently testing something else. Every SQL body is byte-identical. Activation remains. |
| 06 | ReDimNet2-B3 replaces WeSpeaker. **Adopted 2026-09-17 (Q226), the user's decision on the measured record; the registry entry is the change and the front end now rides on the identity.** Measurement had been complete: both halves, plus the split-model architecture on dev (Q180) and on held-out test at declared points (Q183). No measurement work remains. The decision is the user's and is deliberately held open. See below. |
| 07 | Recognition thresholds are re-derived. Dev curve and held-out validation done, now also across both clustering arms and both embeddings at three declared points. No point selected — not because the DER bar is missing, which a threshold does not need, but because ranking the four recognition quantities against each other needs a rate of exchange between a correct and a wrong attributed second, and that is the user's. |
| 12 | A model change re-runs History. Built: `cluster::claims` (`058bcad`), the queue's recording pause and its persistence-boundary stop (Q190/Q192), the additive `rerun` status block plus `diarize/rerunCancel`, the Registry's re-run surface (`4d403d7`), and the bounded seeding writer `diarize::reseed` (`b554fe0`, `04c9de6`, `e4687f6`). and the caller (Q217), which prepares `plan` before inference, embeds the ranges with the run's own embedder, and commits **inside `finish_run`'s attribution transaction, before `persist`** — where the previous attribution is still intact to revalidate against, since `reconcile::apply` overwrites it near the end of that same transaction. Reading and inference stay outside it; the bounded evidence, the attribution and the queue row commit together or not at all. `cluster::Rebuilt` carries the claimed clusters and the re-seeded Speakers through `persist_with`, which would otherwise delete the bounded rows and install the whole-cluster centroid in their place; a claim assigns and never enrols. A `Moved` plan, a failed embedding or an unexpected transaction failure returns `DiarizeOutcome::Owed` — nothing written, row kept, **no attempt budget** — rather than counting an unrecovered Meeting as walked; the bound is the worker's wake-or-thirty-seconds wait, and gone audio is `Skipped` because no later pass grows a recording back (Q218). Claims are settled before matching and their clusters leave the resolve entirely, so a claim cannot take another Speaker's seed, and the bulk path skips the legacy saved-cut rebuild (Q219). Gated on `rerun::is_bulk_work`, false on every History in the field because the tables are unregistered. The startup trigger is wired too (Q221): `Core::rerun_if_the_model_changed` runs at every boot, records the embedding identity on its first start and asks for nothing, and is unreachable in the field because `begin_if_the_model_changed` now answers `None` rather than failing without the tables. Resume and cancel are checked offline through the real completion path (Q222) — an interrupted backlog compared whole against an uninterrupted one, a cancelled one against what the Client is told. **Registering the schema is still what would make any of it fire, and it stays out of `MIGRATIONS`.** `begin` and `begin_if_the_model_changed` stay unreachable and the tables stay unregistered. What remains is activation: the model decision and registration. |

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

**12's groundwork is built and unwired**: `cluster::claims` (`058bcad`,
`c72bb74`) and `store::rerun` with its unregistered tables (`8dd6781`).
Nothing calls any of it.

`claims` is **read-only attribution evidence**. It hands back the clusters a
Speaker owns outright and the Speakers a correction took a whole cluster away
from, reading through `attributed_speaker` and `replaced_speaker` so the
Operator's latest word counts both ways, and excluding the Operator. The rule
on both halves is unanimity, not a vote: anything mixed or unvouched yields
nothing. The test is over the set of owners rather than a count, so splitting
an utterance into more segments cannot change who claims it. An absent claim
withholds the shortcut past the resolve, not the person.

**`cluster::relearn` was built and withdrawn the same day**, and the reason is
worth keeping: unanimity over `reconciliation.assignments` is unanimity among
*transcript segments*, while `live::assemble` builds each cluster's vector
over every grouped `Observation` before reconciliation runs. The vector can
carry speech no segment covers and the parts of each window outside the
segments over it, so "cut entirely from the disputed audio" was never
established — and by the same token a positive claim is not permission to
enrol the raw centroid either. Its dedup was also an existence check rather
than replacement, so correcting away and back left a stale negative. Both
halves of the fix want an embedding bounded to the claimed ranges plus a
stable source identity, which is the seeding path's own work.

`store::rerun` is the backlog state over the existing queue: one row for the
model identity, the size it owns and what cancelling abandoned, plus a
membership table so it counts and cancels **its own** Meetings — `Back` is a
scheduling class and production already enqueues there for Meetings that were
never diarized. The first start records the identity and enqueues nothing;
`begin` is the explicit path a wipe uses and consults no row, so it works with
no prior metadata. Tables unregistered, gate tested beside 05's.

Two ordinary transitions were wrong in the first version and are fixed
(`c175ad0`). Cancelling cleared every membership row, including Meetings it
had just declined to cancel because somebody promoted them to `Front` — they
are still owed, so dropping them made `done` report a promotion as a walk.
And beginning again rebuilt membership from `enqueue`'s answer, which is
`false` for anything already queued, so a second model change mid-backlog
disowned everything the first still had in line. Ownership is now surrendered
for exactly what the queue surrendered, and reconciled rather than rebuilt.

**There is no walker to build.** `Core::run_diarization_queue` already walks
the queue, resumes across restarts and holds the one-at-a-time property; the
re-run's walk is `begin` filling `Back`. What is genuinely missing is the
pause while a Meeting records — the worker never consults `is_recording()`,
so today's catch-up already competes with a live recording — plus the two
additive protocol pieces, the trigger, and the seeding path.

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

**The split-model option is measured on corrected dev** (Q180). The user
authorised the measurement; nothing is adopted. Eight cells — each model's
partition crossed with each model's vectors, in both clustering arms — over the
32-point matcher grid, 256 replays on two inference passes, each cell a **left
join** of identity vectors onto its clustering pass's own observations,
canonical map and turns. A track the identity pass never produced keeps its
timing and its cluster and lends no vector; a cluster left with none gets no
identity embedding, so `assemble`'s `filter_map` drops it from `embeddings`
while its turns survive and the scorer reads it as `unattributed:no-embedding`
rather than the `unexpected` the validity gate watches. Nothing is padded,
fabricated or borrowed from a neighbour.

**Validity, checked before any result was read.** Both diagonals are
event-for-event identical to the standalone single-model ledgers — 6738 events
WeSpeaker-constrained, 6573 ReDimNet2-constrained — against 212 differences
under the withdrawn shared-track restriction. Denominators 22182.515s returning
and 7816.070s new in all eight cells at all 32 points (`unscorable` 1560.070s
excluded); no duplicate rows; `unattributed:unexpected` zero everywhere; all
four DER millisecond tallies equal within each clustering row, asserted by the
rig. DER back to the record: unconstrained 29.13% / 26.01%, constrained 22.48%
— the restriction's 22.46% is gone — and 23.79%. Coverage per pass: ReDimNet2
supplies vectors for 6320 of WeSpeaker's 6329 observations (99.858%, 9
unvectored, 0.8s voiced); WeSpeaker for all 6320 of ReDimNet2's; 9 WeSpeaker
observations have no place in ReDimNet2's partition. **The restriction's damage
is scoped exactly: only the constrained WeSpeaker row moved**, both cells at all
32 points — 64 of 256 replays. The unconstrained WeSpeaker row and all four
ReDimNet2 cells are identical before and after.

**The identity embedding is not close to inert.** That summary was an artefact
of quoting one point. At the shipped matcher point, constrained 0.62/0.08:

| constrained 0.62/0.08 | ret.correct | ret.wrong | new.correct | false-attach |
|---|---|---|---|---|
| We clust / We ident *(control)* | 14030.130 | 2204.195 | 7578.540 | 62.410 |
| We clust / **Re ident** | **16006.440** | 2332.195 | 7524.790 | 116.160 |
| Re clust / **We ident** | 14888.530 | 2165.625 | 7589.680 | 50.720 |
| Re clust / Re ident *(control)* | 16213.960 | 2361.655 | 7565.090 | 75.680 |

Substituting ReDimNet2's identity vectors under WeSpeaker's partition moves
correct returning **+1976.310s** (8.9% of the denominator) and wrong returning
**+128.000s**, with correct-new −53.750s and false attachment +53.750s. That is
a trade, reported with both numbers and **not ranked**: which side is worth more
is a utility judgement the user has not made, and M3's own catalogue holds that
a wrong attribution costs more than an unnamed one. At the same point
Re-clustering with We-identity **dominates the WeSpeaker control on all four**
while trading against the ReDimNet2 control.

**The one dominance over both controls survives on complete inputs** and is no
longer a diagnostic: constrained 0.30/0.00, We clustering with Re identity,
18119.570 / 2801.145 / 7361.380 / 279.570 against controls of 16130.530 /
4426.535 / 6042.470 / 1598.480 and 16680.550 / 4128.995 / 7231.080 / 411.180.
Still the only one in 128 off-diagonal comparisons, and both controls are still
badly calibrated there — WeSpeaker's own best correct returning anywhere in the
constrained arm is the same 18119.570 — so it is a dominance over two poorly
chosen configurations, which is a fact about those configurations rather than
about the models.

**The exact null is real, and it belongs to an arm, not to a partition.**
Re-clustering with We-identity ties its control on all four quantities to the
second at unconstrained 0.80/0.00, 0.62/0.08, 0.45/0.00 and 0.80/0.15 — four
points. Under the constraint the same swap costs 1325.430s at 0.62/0.08.
Separately, We-clustering/Re-identity beats its own control on all four
unconstrained at 0.80/0.00, 0.62/0.08 and 0.80/0.15, each by tens of seconds,
each a trade against the ReDimNet2 control.

**A well-calibrated single model already beats the split's best point.**
WeSpeaker doing both jobs at constrained 0.80/0.00 — 18119.570 / 2645.625 /
7434.140 / 206.810 — dominates We-clustering/Re-identity at 0.30/0.00 on three
of four quantities and ties the fourth. So a win at a badly calibrated setting
does not establish an architectural advantage over calibrating one model, and
any held-out design that reads a split only against its own arm's controls at
its own point cannot see that. This is why the held-out set below runs every
point across both arms.

**No recommendation follows from this, and none is made.** These four are the
recognition column. Cost, stated as harness timing rather than as the
architecture's price: inference was **583s + 606s = 1189s** for the two passes
over 18 meetings, and the grid's other 408s is downstream replay a production
split would not repeat. A production split could share one segmentation pass
between the two embeddings, so 1189s is an upper bound. A split also does *not*
entail two persisted identity spaces per Speaker, since clustering vectors can
stay meeting-local and never be stored against a Speaker.
The DER column is a separate reading. Dominating both same-model controls is
sufficient evidence of a recognition benefit and **not necessary** for a split to
be worth having; "no split pays" was a product verdict the data never
established and is withdrawn.

**Held-out design, predeclared before any test number** (Q182, amending Q181):
three matcher points — **0.80/0.00** as the main common-operating-point
comparison, **0.62/0.08** as the shipped matcher comparison, **0.30/0.00** as a
secondary low-floor diagnostic — across **both** arms and all four model
pairings. 24 configuration replays through the existing
`EVERTRANSCRIPT_MATCHER_POINTS` mechanism, every merge threshold as dev fixed
it. Running both arms is the correction that matters: it puts each split beside
a competitively calibrated same-model cell instead of only beside its own arm's
controls. Two Q181 claims withdrawn: a held-out failure fails to replicate *that
configuration on this split and corpus*, not "no split dominates anywhere"; and
Q153's one failed transfer does not establish that dev-best points generally
fail to transfer, nor justify excluding a competitive control.

### Held-out AMI test: the 24 predeclared configurations (Q183)

**All 24 rows are in [`heldout-24.tsv`](heldout-24.tsv)** — arm, clustering model,
identity model, merge threshold, matcher point, DER, and the four ledger
quantities — aggregated from the saved event files so the numbers outlive
`/tmp`. What follows picks out the comparisons that say something; the table is
the record. Every claim below is a subtraction of two of its rows.


**Validity.** Denominators 25538.370s returning and 5175.554s new, fixed across
all eight cells and three points. No duplicate keys. `unattributed:unexpected`
zero, and `unattributed:no-embedding` **zero** — with 4 of WeSpeaker's 5666
observations unvectored (0.3s) no cluster was left without a centroid, so the
abstention path built for that case was not exercised on this corpus. Coverage:
ReDimNet2 supplies 5662 of WeSpeaker's 5666 (99.929%); WeSpeaker all 5662 of
ReDimNet2's; 4 WeSpeaker observations have no place in ReDimNet2's partition.
Six exact checks against saved standalone test ledgers, all **identical**: both
constrained diagonals at 0.62/0.08 and 0.80/0.00, both unconstrained diagonals
at 0.62/0.08. DER unchanged from the record — 28.64% / 24.62% unconstrained,
23.51% / 23.94% constrained — and equal on all four millisecond tallies within
each clustering row. Inference 651s + 558s over 16 meetings; snapshots kept.

**The dev dominance did not replicate.** Constrained 0.30/0.00,
WeSpeaker-clustering with ReDimNet2 identity, dominated both controls on dev. On
test it is a **trade** against the WeSpeaker control (correct −113.790, wrong
+120.910, correct-new +56.130, false attachment −56.130) and **dominated** by the
ReDimNet2 control (correct −2272.550, wrong +1920.600). That is a failure to
replicate this configuration on this split and corpus, and not a result about
other configurations.

**Zero of the 12 splits dominate both same-model controls of their own arm and
point** — against one in 128 on dev. Against own-partition control, which is the
DER-neutral comparison: 2 dominate, 1 ties exactly, 2 are dominated, 7 trade.

**At the shipped matcher point the sign of the trade changed, which means the
dominance did not replicate either.** Constrained 0.62/0.08, holding WeSpeaker's
partition (the best DER measured here, 23.51%) and substituting ReDimNet2's
identity vectors **dominates its own-partition control on all four** —
18116.280 / 4108.840 / 4960.354 / 0.000 against 17039.900 / 4328.150 /
4947.024 / 13.330, so correct +1076.380, wrong −219.310, correct-new +13.330,
false attachment −13.330, at identical DER. But on dev this cell was a **trade**
(correct +1976.310, wrong +128.000), so there was no dominance here to replicate:
one setting shows a trade and the other a dominance, and the shape of the result
is what differs between them. What did carry across both is the direction of
correct returning, which rose under the swap in each. It still only **trades**
against the ReDimNet2 control in both.

**The exact null replicated.** Unconstrained 0.80/0.00, ReDimNet2 clustering
with WeSpeaker identity ties its control on all four quantities to the second,
as on dev.

**At the main common operating point under the constraint the two splits go
opposite ways, so neither "the split" nor "the architecture" is the subject.**
Constrained 0.80/0.00, correct-new is 4960.354s and false attachment 0.000s in
all four cells, so the comparison is the returning pair alone.
**We-clustering/Re-identity** is the one dominated by both controls: wrong
+141.500 with the other three identical against We/We, and correct −731.300 /
wrong +338.520 against Re/Re. **Re-clustering/We-identity**, at the same point,
**dominates We/We** (correct +687.520, wrong −283.880) and is a **trade** against
Re/Re, giving up 43.780s of correct returning for 86.860s less wrong.

**No split dominates every measured same-model configuration.** Counting
dominances over the 12 same-model cells, the highest count any split reaches is 6;
several reach none. That is a fact about this grid and does **not** imply that no
split beats a calibrated single model — the two claims are different, and the
second is false here. Unconstrained 0.80/0.00, We-clustering/Re-identity beats
We/We at the same point on the one quantity that moves and ties the other three:
wrong returning 2401.140 against 2436.900, so 35.760s less wrong with correct
returning, correct-new and false attachment identical. The same cell at the same
point on dev is also a dominance (wrong −15.550s, correct-new +6.890s, false
attachment −7.590s, correct returning identical), which makes it the one gain in
this work that replicated. It is small, it is one point in one arm, and it says
nothing about any other cell. No cell is called "the best split" here without the
metric that ranks it, because the four quantities do not agree on an order.

**The tension the split does not resolve, at the shipped point:**

| 0.62/0.08 | DER | ret.correct | ret.wrong | new.correct | false-attach |
|---|---|---|---|---|---|
| const We/We *(control)* | 23.51% | 17039.900 | 4328.150 | 4947.024 | 13.330 |
| const We/**Re** *(split)* | 23.51% | 18116.280 | 4108.840 | 4960.354 | 0.000 |
| uncon Re/Re *(control)* | 24.62% | 20628.360 | 2927.080 | 4677.384 | 0.000 |

The best DER measured is constrained WeSpeaker at 23.51%, and under that
partition the split buys 1076.380s of correct returning and less wrong at no
cost on any of the four. Unconstrained ReDimNet2 alone has 2512.080s more
correct returning and 1181.760s less wrong than that split, for 1.11 more DER
points and 282.970s less correct-new. Which of those is preferable is the
undecided adoption bar plus an unchosen rate of exchange, and neither is ours.

**One decision is the user's and is deliberately held open by them:** whether
≥ 2.0 points of DER is the adoption bar. They were asked and chose to leave it
undecided; it is not an unanswered question, and it is **not a premise the rest
waits on** — choosing a recognition threshold does not logically require a
number for the DER bar, and the sentences above that made the two read as one
condition were wrong to. What is actually unresolved are two product choices:
which model and configuration to adopt, and which recognition outcome to
prioritise. **The second needs** the rate of exchange between a correct and a
wrong attributed second, without which the recognition column cannot be
collapsed to one ranking. Nothing here assigns any of these. Q152's rule holds throughout: measurements, mechanism hypotheses
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
and false alarm survive it untouched and only labelling error is removed. Those
spans are turns reconstructed *after* clustering, so it is not invariant to the
clusterer: a different partition can change the per-cell vote and move the
spans the floor is then scored over. Two arms that differ only in clustering
can still show a small difference here, and one does not prove the arms
otherwise identical. It is
therefore not an embedding ceiling, and it licenses exactly one sentence about
a bar below it: **these fixed spans cannot reach that bar by oracle relabelling
alone.** Which stage would have to change to move them is a separate question
this number does not answer. It also flatters
recognition badly if read that way: its centroids are one per person **per
whole meeting**, minutes of speech each, where real enrollment mints a Speaker
from as little as `MIN_SPEAKER_MS` — ten seconds. A separability measured at
whole-meeting duration says nothing about what a ten-second cluster can do.

### Reproducing

Models in one directory. Since the adoption (Q226) the registry names
ReDimNet2-B3 under `diarize-embedding.onnx` and the harness follows it —
`EVERTRANSCRIPT_EMBEDDING` defaults to `redimnet2`, and `wespeaker` now
reads `diarize-embedding-wespeaker.onnx`, so a directory laid out for the
earlier runs needs both files renamed:

```
diarize-embedding.onnx             ReDimNet2-B3            18,045,013   sha256 dcecdce7…6d41
diarize-embedding-wespeaker.onnx   WeSpeaker ResNet34-LM   26,535,549   sha256 3955447b…fcbb
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

`Q135` and `Q145` in the **original** branch's journal — read it at the tag
`archive/diarization-pyannote-redimnet2`; the branch was deleted 2026-09-17 —
remain escalated and were not carried over.
That journal forked from main's at Q115 — both sides claim Q115–Q124 for
different decisions — so it cannot be merged, only read.
