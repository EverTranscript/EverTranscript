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
| 04 | `cfd1077` | A model has an identity — from the registry, stamped by whichever model ran, checked before any match |

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
| 03 | Turns come from segmentation. **Closed (Q210, corrected by Q211/Q212): main's placement already had it.** Main reached the ticket's substance another way — `masks` keeps which local speaker, each is embedded with the others masked out, overlap yields an Observation each, and `MIN_EMBED_FRAMES` is separate from `MIN_SPAN_MS`. Only the sliding step was missing. The grid reconstruction landed (`9381f57`); quantization and its downstream effects explain the measured delta — 0.06 points of pooled oracle floor and 0.2 on one meeting, confirmed by rescoring the same observations at `GRID_MS = 1`, which reproduces the pre-Step-A build exactly. It is not merely that surviving edges move by up to half a cell: rounding both edges feeds `merge_adjacent`, whose 400 ms gap rule can then decide differently. `[0, 1000)` and `[1404, 2000)` are 404 ms apart and stay separate; rounded to the grid the gap is 400 ms and they join. So there is no general “changes no decision at `step == window`” guarantee to claim — only a small measured difference with a known mechanism. The step itself does not pay on the criterion the plan fixed in advance: **0.48 points** of oracle floor at 1 s against a 2-point bar. `SEGMENT_STEP` stays `SEGMENT_WINDOW`. Two defects found in review were repaired before the numbers were trusted (Q211): `assemble` let one window vote twice for a voice it had split across two local tracks, and both same-window diagnostics inferred an observation's source window geometrically instead of reading the recorded one. Corrected, the same-window merge rate is 0.47% → 0.89% → 1.08%, not the twenty-fold rise first reported. The plan's watch item — same-window cannot-link — was **measured and is not worth enabling at the settings tested** (Q214 as corrected by Q216). Enforcing it drives same-window violations to exactly 0 at every step and makes DER **worse** at every step: +0.43 at 10 s, +1.79 at 2 s, +5.70 at 1 s (23.88% → 29.58%, with IB4003 15.1% → 31.6%). Of the same-window pairs with known reference labels, about a third carry the *same* dominant label — 369/1227 at 10 s, 1913/6086 at 2 s, 3770/12189 at 1 s — so the premise that two local tracks of one window are different people does not hold generally. Those counts are descriptive: they count forbidden pairs whether or not the unconstrained run merged them, so they are not lost merge decisions, and they do not divide against the 91 prevented merges to give a ratio or to explain the DER rise, which the decomposition does not attribute. How many of them the unconstrained run would in fact have merged was not measured. **This is six meetings at a fixed merge threshold of 0.60** and rejects enabling the constraint there; it is not a general verdict and does not override the earlier dev-tuned 0.10 and held-out constrained results. Adoption is the user's. One finding is left open for the user, not closed by this ticket: the slide cuts end-to-end DER by ~6 points (29.85% → 23.88%) through falling confusion rather than placement, at 9.3× the segmentation inference. Why confusion falls is untested. **Answered 2026-09-17 (Q253), and the verdict above is unchanged.** Measured on the *current* shipping configuration — ReDimNet2-B3 since Q226, all 16 AMI meetings — so the absolute numbers are not the WeSpeaker-era ones above. **The characterization reproduces:** 10 s → 1 s is −4.31 DER (24.60 → 20.29), and it is confusion (−5.02) rather than placement, the oracle floor moving only −0.71. Coverage-corrected — speech the hypothesis misses cannot be scored as confused, and `covered = total − missed` exactly — confusion on covered speech falls 11.12% → 9.13% → 5.66% → 5.59%; between 2.47 points (a bound assuming every newly-missed millisecond had been fully confused) and 4.89 points of the raw fall survive. **Two things are new.** It **saturates at 2 s**: 99% of the fall is there and the last doubling of window count buys 0.07, so 5× the inference buys everything 9.3× does. And **5 s is pathological**, DER 4.24 *worse* than 10 s on a false-alarm rise of +9.90 while the oracle floor stays flat — not extra speech found, but one speaker fragmented across clusters simultaneously active, which the scorer charges as extra speakers. **Why confusion falls: not grid alignment.** A phase sweep (`with_phase`, harness-only, production 0) ran ten alignments at the production step, window count identical throughout, phase 0 reproducing the baseline exactly. The spread is DER range 0.45 (sd 0.13) and covered-confusion range 0.60 (sd 0.18); the best alignment recovers **5%** of the 5.45-point fall. Aliasing is refuted and observation count is what remains — refuted directly, quantity by elimination plus the monotone step trend. `SEGMENT_STEP` stays `SEGMENT_WINDOW`: the 2-point oracle-floor bar the plan fixed in advance still is not met, the floor moving 0.71. |
| 05 | A model change clears Voiceprints. **Done: registered 2026-09-17 as `MODEL_CHANGE_WIPE`, migration 15, on the user's instruction (Q228).** Registry messaging is done — the wipe keeps the model stamp so a named Speaker can say *why* it is empty (Q184, `3d65e16`). The migration-index criterion is closed ahead of registration rather than after it (Q223): all fourteen migrations are named constants and the three upgrade-path tests derive their index from the migration they mean, so an insertion anywhere but the end cannot leave them silently testing something else. Every SQL body is byte-identical; the wipe's still is. The file-backed test now runs the real upgrade — a WeSpeaker-stamped History migrated to `before(MODEL_CHANGE_WIPE)`, then `migrate` — and asserts the record survives, every vector is gone, and the re-run's stamp is there. |
| 06 | ReDimNet2-B3 replaces WeSpeaker. **Adopted 2026-09-17 (Q226), the user's decision on the measured record; the registry entry is the change and the front end now rides on the identity.** Measurement had been complete: both halves, plus the split-model architecture on dev (Q180) and on held-out test at declared points (Q183). No measurement work remains, and the decision is made; what the record below sets out is the evidence it rests on. The interval before the wipe is registered runs the lazy rebuild on the old model's cuts (Q227), not the policy. |
| 07 | Recognition thresholds are re-derived. **Done 2026-09-17 (Q231): the point stays 0.62/0.08**, selected from Q182's predeclared points as run in Q183, read in the shipping arm only (unconstrained, ReDimNet2 clustering and identity, held-out test) — no new run and no retuning. 0.30/0.00 is dominated on all four. Against 0.80/0.00 it is a trade, +303.790s correct returning for +37.700s wrong with newcomers tied, decided at an assumed rate of exchange of 4 correct seconds per wrong second; break-even is 8.06, so the ranking holds for any rate under that. `MATCH_FLOOR`/`MATCH_MARGIN` unchanged — by selection now, not inheritance. The DER bar is answered separately and not as a number (Q232). |
| 12 | A model change re-runs History. Built: `cluster::claims` (`2880ba6`), the queue's recording pause and its persistence-boundary stop (Q190/Q192), the additive `rerun` status block plus `diarize/rerunCancel`, the Registry's re-run surface (`4d403d7`), and the bounded seeding writer `diarize::reseed` (`b554fe0`, `04c9de6`, `e4687f6`). and the caller (Q217), which prepares `plan` before inference, embeds the ranges with the run's own embedder, and commits **inside `finish_run`'s attribution transaction, before `persist`** — where the previous attribution is still intact to revalidate against, since `reconcile::apply` overwrites it near the end of that same transaction. Reading and inference stay outside it; the bounded evidence, the attribution and the queue row commit together or not at all. `cluster::Rebuilt` carries the claimed clusters and the re-seeded Speakers through `persist_with`, which would otherwise delete the bounded rows and install the whole-cluster centroid in their place; a claim assigns and never enrols. A `Moved` plan, a failed embedding or an unexpected transaction failure returns `DiarizeOutcome::Owed` — nothing written, row kept, **no attempt budget** — rather than counting an unrecovered Meeting as walked; the bound is the worker's wake-or-thirty-seconds wait, and gone audio is `Skipped` because no later pass grows a recording back (Q218). Claims are settled before matching and their clusters leave the resolve entirely, so a claim cannot take another Speaker's seed, and the bulk path skips the legacy saved-cut rebuild (Q219). Gated on `rerun::is_bulk_work`, false for every Meeting the backlog does not own. The startup trigger is wired too (Q221): `Core::rerun_if_the_model_changed` runs at every boot, records the embedding identity on a History with no row and asks for nothing, and answers `None` rather than failing on one without the tables. Resume and cancel are checked offline through the real completion path (Q222) — an interrupted backlog compared whole against an uninterrupted one, a cancelled one against what the Client is told. **Done: registered 2026-09-17 as `MODEL_CHANGE_RERUN`, migration 16, directly behind the wipe (Q228).** Registration alone would have left a wiped History that the gate read as a first start (Q221's guard) — no walk, and nothing for the lazy path to rebuild — so the migration seeds `diarize_rerun` with WeSpeaker's identity at total 0: a stamp, not a backlog (`requested()` false), that the next start reads as a model change. Pinned at three levels: `schema` (the row after the real upgrade), `rerun` (the stamp makes the first start `Some(3)` oldest-first; a fresh install `Some(0)` then `None`), and `server` (`Core::new` + `rerun_if_the_model_changed` on a populated History enqueues all of it, and `Some(0)` is silent). Removing the seed fails four tests. **The last criterion is closed (Q234):** run 2026-09-17 on `macbook-pro-nickel` against a real 361M History, backed up whole beforehand to `~/EverTranscript-backups/pre-migration-20260917-045650/`. The gate wiped and enqueued on first boot, the walk took sixteen minutes and finished 10 of 10 with 0 abandoned and 0 owed, and `user_version` went 14 to 16. Every Voiceprint is now stamped `redimnet2-b3`/`1`; attribution rose 2584 to 2593 of 3744 segments and exemplars 78 to 1458. That History holds **zero `attribution_hints`**, so the recognition result is unmixed with any Operator correction: Marc Ammann earned a Voiceprint from 2026-09-08 audio and the machine attributed him 23 segments of the 2026-09-16 Meeting eight Meetings later, on the vector alone. `rerunError` stayed absent on `diarize/status` throughout. Six of the eight named Speakers came back with a Voiceprint; the two that did not are the Operator, excluded from relearning by design, and a Speaker with no attributed audio left to learn from. **The Operator's exclusion had a measured cost:** 366 segments across 10 Meetings became 483 across 6, so their own name went missing from the four most recent Meetings (Q236). **Fixed in Q237** by removing the exclusion from `reseed::plan` and `cluster::claims`. **Verified in Q243**, on the same host and on a copy of the same History, driven through `diarize/rerunRequest` rather than an installed build — no install has happened, Q235 is still open. The Operator earns a `redimnet2-b3`/`1` Voiceprint back, so 7 of 8 named Speakers hold one. **The four lost Meetings do not return:** the Operator stood at 6 Meetings before the second walk and 6 after, and in each of the four a pseudonym holding a Voiceprint cut from exactly those segments keeps them. Removing the exclusion restores the Operator's eligibility to hold a biometric; it does not recover speech a pseudonym already owns, which needs a re-enrol or merge surface that does not exist. **All criteria are met, but verifying them found a defect outside them:** the Voiceprint mint has no exemplar-agreement check, and on this History the Operator's relearned Voiceprint matches Ming Chen rather than the Operator (Q246–Q249) — see *Escalated, still open* and ticket 13, held for user. |

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

**05's migration is written and tested, and since 2026-09-17 registered (Q228).**
Before that `schema::PENDING_MODEL_CHANGE_WIPE` sat beside `MIGRATIONS` and outside it,
with three tests: a file-backed control that an ordinary open leaves a current
History alone, a file-backed close/reopen that the wipe takes every vector and
keeps every Speaker, name, flag, mark, attribution and hint, and one that
asserted it was still unregistered — since appending it to `MIGRATIONS` is the
whole of activating it; that one now asserts the wipe and the re-run are
registered as a pair. `stale_exemplars` is empty afterwards for the current identity
and for a hypothetical next model, which is what stops the lazy rebuild path
reintroducing the old model's cuts behind the wipe. **05 is not done**: the
Registry messaging and the activation remain.

**12's groundwork was built and unwired**: `cluster::claims` (`2880ba6`,
`ff74f59`) and `store::rerun` with its then-unregistered tables (`f2d9b48`).
The rows above say where it stands now.

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
no prior metadata. Tables registered 2026-09-17 (Q228), gate tested beside 05's.

Two ordinary transitions were wrong in the first version and are fixed
(`cbc82a5`). Cancelling cleared every membership row, including Meetings it
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
rules responsible. **Measured 2026-09-17 (Q234) and it did not hold as written:**
the third channel rule *is* the Voiceprint match, and nothing rebuilt the
Voiceprint it needs, so a model change left only the isolated-mic and dominance
rules standing. On the real History that cost the Operator their name on the four
most recent Meetings. **Resolved by the user as Q237:** the trap is retired and the
exclusion is gone from both `reseed::plan` and `cluster::claims`. The Operator now
relearns from their own attributed ranges like any other named Speaker; ADR-0029's
three rules still decide who the Operator is, and the boundary that remains is that
only attributed ranges are read, never a cluster vector. Two tests pin it and each
fails if its exclusion is put back.

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

### `Q135` / `Q145`, on the archived branch

`Q135` and `Q145` in the **original** branch's journal — read it at the tag
`archive/diarization-pyannote-redimnet2`; the branch was deleted 2026-09-17 —
remain escalated and were not carried over.
That journal forked from main's at Q115 — both sides claim Q115–Q124 for
different decisions — so it cannot be merged, only read.

### The Voiceprint mint has no agreement check — ticket 13, **not shipping**

**Q257/Q258, 2026-09-17: the defect is upstream, and ticket 15 is where it
goes.** The user ruled AMI's overlap rate representative and deferred the
guard, so nothing from ticket 13 ships; `split`, `Split::is_two_voices`,
`AGREEMENT_FLOOR` and `MINORITY_SHARE` stay on `main` as measurement surface,
read by nothing. The tell all three measures kept producing: **strict
attribution passes the same partition test 16 of 16.** The pools are the
defect, not the mint. `reseed::plan` enrols whole transcript segments including
mic windows the far end talked over, and the product has both channels'
transcript segments, so the purity signal AMI-strict approximates is a query
here — see *An exemplar is cut from a window the other channel was talking
over*, below. Ticket 13 stays open only for the residue no window filter can
see: two people on one far-end call, recorded on one channel.

**Q256, 2026-09-17: a third measure, built and not shipped.** The user's
two-way partition test — refuse when the best two-way split's group centroids
score under 0.50 *and* the minority holds a quarter of the exemplars — clears
**five of six** predeclared criteria: the Operator's 9 refused at −0.0923, all
six named controls minting, 4:5 and 3:6 refused, 2:7 passing by design, AMI 16
refusing 0 under strict attribution. It fails the sixth: under permissive
attribution — which `reseed.rs` matches, since it cuts one whole transcript
segment per exemplar — **10 of AMI's 16 real people are refused**. The open
question is which corpus's overlap rate represents the product's users, which
is a judgement about users rather than code. `split` is on `main` as
measurement surface; the mint is unchanged. Distribution in the ticket file.

**Q255, 2026-09-17: the first measure was built, failed its AMI gate, and was
withdrawn before shipping.** `agreement` (mean pairwise cosine over the
post-14 selected exemplars) and `AGREEMENT_FLOOR = 0.50` are on `main`;
`refresh_voiceprint` does not read them and mints exactly as before. AMI's
sixteen reference speakers are sixteen real people and **seven score under
0.50** in the shape the guard binds on, down to 0.3759 — or all sixteen down
to 0.2081 if windows are attributed without requiring them to be
single-voice, which is *below* the contaminated Operator's 0.3190. No
threshold move rescues it. Averaging each Meeting's exemplars first does
separate on both corpora at that same 0.50, joint window (0.3597, 0.6767) —
but that is a change of measure, so **the choice list is four items now, not
three**. Full distribution and the two blind spots in the ticket file.

**Q246–Q249, 2026-09-17.** Found while verifying ticket 12, not by that
ticket's own criteria, which is why 12 reads as met above and this is separate.
Ticket file: `issues/13-a-voiceprint-is-not-minted-from-exemplars-that-disagree.md`.

`cluster::centroid` averages a Speaker's exemplars and tests nothing about
whether they are the same voice, and `refresh_voiceprint` stamps the result
with that Speaker's name. So **relearn will mint a Voiceprint from mutually
orthogonal exemplars** (Q246). The same mint serves every Speaker on every
host — this is not a `macbook-pro-nickel` finding.

It is not hypothetical. On the real History, Q237's relearn gave the Operator a
Voiceprint scoring **0.6761 / 0.6724** against the two `Ming Chen` rows —
above the 0.62 match floor — and **0.3473** against their own clean voice
(Q248). The walk log shows it already attributing speech
(`operator_rule=Discriminant(2)`), and ADR-0008 as amended lets a confirmed
Voiceprint outrank an unconfirmed one, so it wins ties it should lose.

`MAX_EXEMPLARS = 32` is the mechanism, and it does not sample: exemplars are
read `ORDER BY id` over UUIDv7 ids and `centroid` takes `.rev().take(32)`, so
the newest 32 are the contiguous **tail of the last Meeting that contributed**.
The weighted centroid of those 32 reproduces the stored vector at `1.000000`
against `0.516952` for all 483 rows; all 32 came from one Meeting and all 32
overlap system-channel speech (Q249). One Meeting's ending decided an identity.

**The guard is named and deliberately not built** (Q246): one check at the mint
site, falling through to the `clear_voiceprint` branch that already exists.
Three choices are the user's, and the first gates the rest:

| # | Choice | What the measurement says |
|---|---|---|
| 1 | **The threshold** | On mean pairwise cosine. **Re-measured post-14 (Q254): (0.3190, 0.5867)**, and the user's predeclared 0.50 is inside it. The earlier readings — (0.4426, 0.6143) capped-32 and (0.3674, 0.6143) whole-record — were taken on the Operator's pre-relearn 483 rows; they now hold 9, so both readings coincide at 0.3190. Upper bound is the weakest named control throughout |
| 2 | **Refuse, or keep the coherent subset** | Refuse is minimal and loses recognition; subset loses less and is no longer one line. Q248 argues a blended Voiceprint is confidently wrong rather than vague — a reason to fail closed, not a decision to |
| 3 | **Capped 32, or the whole record** | **Settled by the user: the post-14 selection** — one read, one path, no separate whole-record score. Moot for the Operator on today's History anyway, whose 9 rows are under the cap |

`min` exemplar-vs-centroid — the measure the guard as first named would most
naturally use — **inverts** and must not be used: the Operator scores 0.5142
and the weakest control 0.4170 (Q247).

All of it is one History of ten Meetings: evidence that a workable threshold
exists, not a measurement of where it generalises.

Ticket **14** below is the companion defect and moves this band: see there.

### An exemplar is cut from a window the other channel was talking over — ticket 15, **built mic-only and measured on a rebuild (Q261)**

Ticket file:
`issues/15-an-exemplar-is-cut-from-a-window-the-other-channel-was-talking-over.md`,
all six acceptance criteria met. Measured read-only on the real History
*before* anything was built, as instructed; then built **mic-only** on the
user's ruling (`ea13e2b`) and measured again on a rebuilt copy (Q261). The
rule: drop a **mic** exemplar window when the **system** channel carries voiced
speech intersecting it, any intersection, no threshold. Not adopted for the
real History.

**What the rebuild showed (Q261).** The rule is exact on the path that ships
it: reseed-shaped mic rows overlapping the far end go **27 of 27 → 0 of 133**,
and the exemplar corpus *grows*, 1472 → 1776. The Operator ends with **139**
exemplars and a Voiceprint at **0.9960** against their own clean voice where
they had **none at all**, attributed in all 12 Meetings instead of 6. Five
named controls keep their identity (0.9536–1.0000); `Menggang Xu` is stranded
with `forgotten = 0`, so named-with-a-vector stays 6 and the membership trades
their recognition for the Operator's.

**The cost, classified (Q263).** 265 segments lose their owner, and **every one
is mic audio the system channel was talking over** — 2963 s of double-talk,
**zero clean segments**, and **none owned by a named Speaker or the Operator**.
The 20 → 11 fall in pseudonyms holding a vector is a net: **13 lost, 4 newly
minted**. Eleven of the 13 held one exemplar in one Meeting and none matched a
known voice above `MATCH_FLOOR` (best 0.5370), so they were unrecognisable
fragments rather than voices; ten are mic-only and the rule's doing, three are
system-channel and therefore re-run churn. 265 is still an upper bound, because
a bulk re-run re-derives attribution anyway.

**Two corrections the rebuild forced (Q260).** `named` is not a synonym for
`reseed-shaped`: 64 of the 1472 rows are machine-written, including **all nine
of the Operator's**, so the pre-build numbers below measured a coarse
per-Meeting proxy for the Operator rather than the per-window rule. Tell the
writers apart by `voiced_ms == sample_end_ms - sample_start_ms` — true only of
a reseed row, whose vector *is* its window. The pre-build figures that follow
are kept as written, with that caveat.

**It repairs the Operator's record rather than refusing it** — which is what
neither of ticket 13's measures could do. All nine of the Operator's usable
exemplars are on mic; six overlap system speech and go. The surviving three
score **0.9811** against the Operator's own clean voice (`d859b1`), where the
blend of all nine sat at **0.6183**, and the far-end Ming Chen rows stay at
0.3283/0.3164. The two centroids agree with each other at only 0.6227, so it is
a different vector and not a rounding.

**Five of the six named controls keep their identity and one is stranded.**
Survival and the surviving centroid against the Voiceprint each carries today:
Hong Li 48 of 153 (0.9443), Jack Ahn 160 of 890 (0.9713), Marc Ammann 29 of 59
(0.9827), Ming Chen 36 of 79 (0.9701), Ming Chen 70 of 230 (0.8654).
**`Menggang Xu` holds exactly one exemplar, on mic, overlapped — so the rule
leaves them nothing and their Voiceprint is cleared.** Reported rather than
softened, as instructed; clearing sets no forgotten mark, so the name and
identity survive and only recognition is lost. Across every Speaker: 1472
usable exemplars, **1115 dropped (76%)**, 15 Speakers left with none — that one
named Speaker and fourteen pseudonyms.

**The machine path does not select windows this way**, so the rule cannot be
applied to its exemplar: its vector is the cluster centroid over every
observation, and `sample` is a playback pointer taken from the middle of the
longest *clean* run (`live.rs:858`). The filter belongs on the observations
before `centroid` consumes them (`live.rs:865`). That path already computes
`clean_runs`, but same-channel only, so it says nothing about the far end.

**One thing is reported and not acted on (Q258, escalated).** The specified
rule is symmetric; the channels are not. The mic recording can carry far-end
speech leaking in from the speakers — Q246's path. The system recording is a
tap of the output stream, so the Operator's voice has no route into it.
Measuring the mic-only variant: the Operator's repair is **identical** (3 kept,
0.9811, because all nine of their exemplars are on mic), while the controls
keep 153/153, 866/890, 58/59, 79/79 and 230/230 — a ~2% drop rate instead of
76%, since 1372 of their 1412 exemplars are system-channel. `Menggang Xu` is
stranded under both. Whether the rule is symmetric or mic-only is a scoping
call about which capture paths can carry contamination, and it is the user's.

### The exemplar cap took a tail, not a sample — ticket 14, **built (Q254)**

**Shipped 2026-09-17**, `cluster::spread_across_meetings` called from
`refresh_voiceprint`: round-robin across contributing Meetings, newest Meeting
first, newest-first within each, until `MAX_EXEMPLARS`. The user chose the
rule; no new constants, the cap stays 32. On the real History three Speakers
now draw on more Meetings than the tail gave them (Jack Ahn 3 → 9, Hong Li
2 → 5, Ming Chen 3 → 5) and the other 24 are under the cap and untouched. It
**narrowed ticket 13's band to (0.3190, 0.5867)** — see that subsection. What
follows is the diagnosis as written before the fix; its absolute counts predate
two Meetings recorded 2026-09-17 17:32.

**Q251, 2026-09-17.** The companion defect to 13, in the same mint and
independent of it. `centroid` takes `.rev().take(32)` over ids that are UUIDv7
minted at insert, so the cap is a **contiguous tail** — the end of the last
Meeting that contributed. Ticket file:
`issues/14-the-exemplar-cap-samples-a-speaker-rather-than-taking-a-tail.md`.

Measured read-only on the real History. Five Speakers exceed the cap, so those
are the only rows where it binds:

| Speaker | usable | Meetings | tail-32 spans | spread-32 spans |
|---|---|---|---|---|
| Jack Ahn | 888 | 7 | **1** | 7 |
| Ming Chen | 229 | 4 | **2** | 4 |
| Hong Li | 152 | 4 | **1** | 4 |
| Ming Chen (dup) | 79 | 1 | 1 | 1 |
| Marc Ammann | 58 | 2 | 2 | 2 |

**This one needs no threshold** — it is a selection rule, not a policy — so
unlike 13 it is buildable as soon as the rule is picked. Three findings temper
that: on this History spreading changes **no** identity (nobody matches anybody
else at 0.62 under any of the three selections); it **lowers** coherence in
every case, because breadth means different rooms and microphones; and it
therefore **narrows ticket 13's band**, because the weakest control — the
229-exemplar `Ming Chen` at 0.6143, which *is* that band's upper bound — drops
to 0.5846 when spread. Whichever of 13 and 14 lands second must re-measure.

The Operator is where it mattered, and it recurs: they are attributed 483
segments across 6 Meetings on the real History today and **the last 32 fall in
`01a08e16` alone**, the same Meeting whose every window carried double-talk. A
relearn run now would concentrate on it again.

## `macbook-pro-nickel`, the host with the real History

The only host with a populated History (`~/Documents/EverTranscript`, 10
Meetings, 8 named Speakers), so anything needing real recorded audio runs
there. It is a laptop that sleeps, surfacing for seconds at a time — which is
why remote work on it is one self-contained script fed over
`ssh … 'bash -s'` rather than several round trips.

**The installed app is a build from `main` since 2026-09-17 15:35:52, and the
"must not start" hazard is gone (Q259).** It read, until that install: *"the
installed app is still v1.1.1 and must not start"* — because Q234's migration
left ReDimNet2 at `diarize-embedding.onnx`, 18 045 013 bytes, under the fixed
filename the WeSpeaker-era build expects 26 535 549 at, so that build reads it
as `Corrupted` on size and re-fetches WeSpeaker over it, after which Q227's
lazy path rebuilds every Voiceprint back into the old space.

Q235's attended install is what closed it. Two Meetings were then recorded on
that machine — 15:37:21 and 17:32:24 — and the hazard did **not** fire:
`diarize-embedding.onnx` is still 18 045 013 bytes / `dcecdce7…`, matching
`registry.rs:281`; the WeSpeaker sibling is untouched since 09-05; all 26
stored Voiceprints and all 1472 exemplars are stamped `redimnet2-b3`/`1`, with
no WeSpeaker rows and no mixed space; and the unified log shows no fetch.

**The version string was never the identifier, which is the flaw in the old
warning.** `Cargo.toml:16` puts `main` at `1.1.1` too, so "v1.1.1" named both
the dangerous bundle and its replacement. What distinguishes them is the
binary. `Contents/Resources/evertranscript`, mtime 15:37:07, embeds its own
model registry, and the decisive string is the **expected hash**:

```
strings -a /Applications/EverTranscript.app/Contents/Resources/evertranscript \
  | grep -c dcecdce7d52bbd4739b24d0874359ec564d43f4b3a392f0104f505593b566d41
```

Non-zero means the installed build expects exactly the ReDimNet2 bytes that are
on disk, so it cannot read them as `Corrupted`. In the same string table that
hash sits beside `diarize-embedding.onnx` and
`soulmachine/evertranscript-redimnet2-b3-vox2-lm`. A `grep -c redimnet2-b3`
returns 9 here, but that counts *lines* and this binary's string table is a
handful of very long ones, so it is a weak signal — use the hash. Identify a
build by the model it expects, not by what it calls itself.

**Runbook: the real re-run, after the attended sign-and-swap (Q267).** The
user decided this happens. The install is theirs — `bash /tmp/et-sign.sh`
(expect `SIGN_OK`; it cannot be done over ssh, see
`local-macos-install-recipe.md`) then `bash /tmp/et-swap.sh`. Everything below
is the next session's, in order. No watcher is armed for it.

1. **Prove the installed binary is the staged one — by the embedded hash, not
   the version string.** Both builds call themselves `1.1.1`.
   ```sh
   shasum -a 256 /Applications/EverTranscript.app/Contents/Resources/evertranscript
   ```
   It must read `325e4b82…`, the bundle staged from `6579ed0`. The pre-swap
   binary is `2a61646e…`; if that is what comes back, the swap did not happen
   and there is nothing to re-run. Confirm the bundle still satisfies the TCC
   requirement — `codesign -d --requirements - /Applications/EverTranscript.app`
   must name `identifier "com.evertranscript.client"` and the
   `Apple Development: Frank Dai (CCDB33UUQ9)` leaf, or the grants are gone and
   the microphone will re-prompt.

2. **Back up and check integrity first, as Q234 did.**
   ```sh
   DB=~/Documents/EverTranscript/.data/EverTranscript.db
   BK=~/Library/Application\ Support/EverTranscript/backups/EverTranscript.db.before-t15-$(date +%Y%m%d-%H%M%S)
   sqlite3 "file:$DB?mode=ro" ".backup '$BK'"
   sqlite3 "$BK" 'PRAGMA integrity_check;'   # must print exactly: ok
   ```
   Record `shasum -a 256 "$DB"` before the request, so the re-run's effect is
   attributable afterwards. `et-swap.sh` also takes its own backup at swap
   time; this one is the re-run's.

3. **Send the request.** Bulk only — `rerun::is_bulk_work` gates a per-Meeting
   `diarize run` *out* of the reseed path, so only this exercises the filter.
   Newline-delimited JSON-RPC on
   `$EVERTRANSCRIPT_RUNTIME_DIR/evertranscript.sock`, and `initialize` must be
   the first request on the connection or the Core answers `-32001`. Then
   `diarize/rerunRequest` with `{}`, and poll `diarize/status` until
   `rerun.remaining == 0`. It took ~18 min for 12 Meetings on the copy.

4. **Expected result — repeat the Q261 measurement against the real record.**
   The Operator mints a Voiceprint near **0.9960** against `d859b1` from ~139
   exemplars, attributed in all 12 Meetings instead of 6. `Menggang Xu` ends
   unrecognised with **`forgotten = 0`** — name and identity intact. About
   **265** mic segments that the system channel talked over lose their
   pseudonym label and go unattributed, and 13 pseudonyms lose vectors while ~4
   are newly minted. Five named controls keep their identity at 0.9536–1.0000.

5. **What would mean stop.** Restore from step 2's backup and do not continue if
   any of these appear: the Operator's Voiceprint lands **below 0.62** against
   `d859b1`, or agrees with a *named* Speaker above that floor — either means it
   minted somebody else. A **named** Speaker other than `Menggang Xu` loses its
   vector, or any Speaker gains `forgotten = 1`. Segments losing an owner run
   far past ~265, or **any clean (non-overlapped) segment loses its owner** —
   Q263 measured that as exactly zero, so a non-zero count means the filter is
   not doing what was measured. Exemplars stamped anything but
   `redimnet2-b3`/`1`, or `diarize-embedding.onnx` moving off 18 045 013 bytes /
   `dcecdce7…`, which would mean a model re-fetch. Also stop if the request is
   refused with `-32001` after `initialize` — that is a protocol mismatch, not
   a retry.

**Its login item is disabled, 2026-09-17 (Q241).**
`~/Library/LaunchAgents/com.evertranscript.core.plist` carried `RunAtLoad` true
on that v1.1.1 binary, so the next login would have done exactly the above with
nobody opening the app. `launchctl disable gui/<uid>/com.evertranscript.core`
now holds it, keyed on the label so it survives a reboot and a rewritten plist;
the plist is copied to
`~/EverTranscript-backups/com.evertranscript.core.plist.disabled-20260917-132237`
and otherwise left in place. Re-enable with `launchctl enable` once a build from
`main` is installed. The embedding file read 18 045 013 bytes before and after.
That build is now installed (above), so the re-enable is unblocked — it has not
been done, and it is the user's call, not a cleanup step.

Its `/tmp` also holds eleven scripts from two earlier sessions **on that
machine** — audited 2026-09-17: none running, none in state `T`, no crontab, no
plist referencing any of them. Four drive the installed Core against the real
History and two replace `/Applications/EverTranscript.app`; the install they
were waiting on has happened, but they were written against the WeSpeaker-era
bundle, so read each one before running it rather than trusting the label. They were kept rather than deleted because two,
`et-sign.sh` and `et-swap.sh`, are cited as the install recipe in that host's own
`local-macos-install-recipe.md`.
