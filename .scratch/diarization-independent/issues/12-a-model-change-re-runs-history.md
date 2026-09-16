# 12: A model change re-runs History

Rewritten 2026-09-16 against `main`. The ticket of the same number on
`diarization-pyannote-redimnet2` is marked done there and did not land here;
what follows replaces it. **The policy is unchanged** — only the starting state
it is written against has moved.

**Parent:** `docs/prd.md` (Speakers & diarization, stories 33b–33i) and
ADR-0037.

**Blocked by:** 05, and nothing else. The branch listed 06, 08, 09, 10 and 11;
08–11 have all landed here, and 06 gates *activation* rather than the work.

**Status:** ready, not started. Not activated — see *Activation* below.

## What to build

The last piece, and the first thing this product does that touches all of
History at once.

After 05's migration has cleared every Voiceprint, a bulk re-run walks History
**oldest first**, one Meeting at a time, and relearns what the Operator already
told the product. In each Meeting a **named, unforgotten** Speaker is seeded
from its own attributed segments embedded by the new model — corrections
winning over the machine's attribution, negatives rebuilt from corrections that
took a segment away — and those windows become its new exemplars. That uses the
Operator's confirmation of a **whole cluster**, which ADR-0009 already puts
outside the machine's reach, and it is the reason ADR-0037 rejected re-embedding
the old model's stored cuts. Pseudonymous Speakers are re-minted and
renumbered. The Operator is rebuilt by the three channel rules alone
(ADR-0029 as amended), never from the previous model's attributions. A Meeting
with no Kept Audio cannot be re-run and keeps the attributions it has.

It is automatic, it enters the queue at the back, it pauses while any Meeting
records, it resumes across restarts, and it is cancellable. Progress lives in
the Voice Registry, because an unannounced multi-hour background job that
reprocesses everything is indistinguishable, from outside, from the product
misbehaving.

## What changed since the branch wrote this

**The premise for `claims` must now be stated rather than assumed.** The branch
justified reading who owned each segment before the run overwrites it by saying
"there is no vector left to seed with". That is true **after 05's wipe** and
false on `main` today, where the lazy rebuild path (Q115) leaves one. Since 12
runs after 05, the justification holds — but it is a consequence of 05 and has
to be written as one, with a test that fails if the wipe is ever reduced to
something that leaves exemplars behind.

**The parts it builds on are here and are worth naming**, since the branch's
version built some of them itself:

- `store::speakers::relearnable` — named-or-Operator, minus the forgotten.
  Landed with ticket 09 (`c58d347`); this is what makes "a deleted Voiceprint
  stays deleted" true of a bulk re-run and not only of the moment of deletion.
- `store::diarize_queue` with `Priority::{Front, Back}`, surviving restarts.
  Landed with ticket 08 (`e3721ae`). The re-run's work goes at `Back`; a
  just-ended Meeting at `Front` does not wait behind it.
- `store::speakers::attributed_speaker` — the display join, newest correction
  wins. This is what `claims` has to read through, not `speaker_id` directly.
- `registry::DIARIZE_EMBEDDING.voiceprint()` — the model identity, from
  ticket 04 (`ba8a491`). Both the trigger and the resume guard: a re-run row
  carrying the model it was started for is what makes resume-not-restart
  structural rather than a flag somebody clears.

**Still to build, and absent from `main`:** `store::rerun` (the single backlog
row: model identity, original size, what cancelling abandoned),
`cluster::claims`, `cluster::relearn`, the optional `rerun` block on
`diarize/status`, and the `diarize/rerunCancel` method. Both protocol changes
are additive (ADR-0028) and a Core with no re-run must encode byte-identically
to today's shape.

## Acceptance criteria

- [ ] A named Speaker is recognized again after its Meetings are re-run, end to
      end from an old-model History
- [ ] Seeding comes from attributed whole clusters, not from the old model's
      stored sample offsets — asserted, since the lazy path next door does the
      opposite and the distinction is the reason this ticket exists
- [ ] A forgotten Speaker is not re-embedded, and a Meeting without Kept Audio
      keeps its attributions
- [ ] Corrections outrank the machine's attribution when seeding, and negatives
      are rebuilt
- [ ] Recording pauses the re-run and it resumes afterwards; a just-ended
      Meeting is diarized ahead of the backlog
- [ ] Quitting mid-run and restarting resumes rather than restarts, and reaches
      the same end state as an uninterrupted run
- [ ] Cancelling stops it, reports honestly how far it got, and leaves every
      already-processed Meeting correct
- [ ] Progress, pause state and cancellation are observable through the
      protocol, additively (ADR-0028)
- [ ] Renumbered pseudonyms and any named Speaker left without a Voiceprint are
      stated to the Operator rather than discovered

## Activation

Same two dependencies as 05, plus one of its own:

1. The model choice (ticket 06) is settled by the user. Both reserved decisions
   — the ≥ 2.0-point bar, and whether to pursue split models — are open.
2. Ticket 05 has landed, since this re-runs into the state its wipe leaves.
3. **A real end-to-end exercise needs the ONNX models and roughly an hour of
   audio per Meeting**, which is why the branch's version shipped with that
   admittedly untested. Driving `claims`/`persist`/`relearn` directly covers
   the seams either side; the middle stays uncovered until someone runs it on a
   populated History, and that is a deliberate gap to state rather than hide.

No part of this may be run against a real History before (1) and (2). Writing
and testing it is safe; adding the trigger is not.

## Built: `cluster::claims` and `store::rerun` (`058bcad`, `c72bb74`, `8dd6781`)

Landed ahead of the rest because none of it adds a protocol method, a
production caller or a registered migration. **Nothing calls any of it**; the
re-run it belongs to is gated behind 05 and the model decision.

### `cluster::claims` — read-only attribution evidence

Reads who owned each segment **before** the run overwrites it — the previous
model's attribution with the Operator's corrections on top — and hands back
two things: the clusters a Speaker owns outright, and the Speakers a
correction took every one of a cluster's segments away from. It reads through
`store::speakers::attributed_speaker` and `store::speakers::replaced_speaker`,
never `speaker_id`, so the Operator's latest word counts in both directions.
The Operator is filtered out; the channel rules own them.

**The rule on both halves is unanimity.** A cluster is claimed only where
every one of its segments belongs to the same eligible Speaker, and denied
only where every one was corrected away from the same Speaker; conflicting or
unsupported ownership yields nothing rather than a winner. This ticket seeds a
named Speaker **from their own attributed segments**, and naming a cluster the
old model drew was never confirmation of every voice in a new, differently
drawn one — the re-run redraws them, so a cluster can arrive holding two
people's words or one person's mixed with audio nobody vouched for. The test
is over the **set** of owners, never a count, so splitting an utterance into
more segments cannot change who claims it; a tally would make transcription
granularity an input to identity, and a two-name tie would be broken by
comparing UUIDs.

**It is evidence about attribution, not permission to enrol a cluster
vector.** `live::assemble` builds each cluster's vector with `centroid` over
every grouped `Observation`, *before* reconciliation maps transcript segments
to clusters. So the vector can carry speech no segment covers at all, and the
parts of each observation window outside the segments over it. Unanimity here
is unanimity among the segments and cannot speak for the rest of the vector —
in either direction. A writer that enrols or suppresses a voice from a claim
needs an embedding **bounded to the claimed ranges**, which this API does not
carry.

**An absent claim is not a lost person.** A claim is the coarse fast path: a
claimed cluster could be assigned rather than resolved. Where it abstains, the
Speaker still has their Voiceprint for the resolve, and the seeding path can
rebuild them from the ranges that *are* theirs.

### Withdrawn: `cluster::relearn`

Built and removed in the same day. It filed a denied cluster's centroid as a
negative exemplar, justified by the unanimity above — which, per the
provenance note, does not establish that the vector is cut from the disputed
audio. Its idempotency was also dedup rather than replacement: correct away
from Alice, relearn, correct back, relearn again, and the stale negative
stays; a repartition writes a second vector and keeps the obsolete one. 05's
one-time wipe does not reach a later correction under the same model.

The negative half comes back with the seeding path, which re-embeds the
claimed ranges and will have both of the missing inputs in hand: a vector
bounded to what the correction actually covered, and a stable source identity
to scope an atomic replacement to.

### `store::rerun` — the backlog state, over the existing queue

One row (`diarize_rerun`): the model identity the backlog is for, the size it
started at, whether the Operator stopped it, and what cancelling abandoned —
without which `done` is `total - remaining` and jumps to `total` the moment
the queue is emptied, telling an Operator who stopped at 1 of 40 that all
forty were walked. `begin`, `begin_if_the_model_changed`, `state`, `cancel`.

A second table (`diarize_rerun_backlog`) records **which Meetings are the
re-run's own**, because the queue's `Back` priority is a scheduling class, not
a job: production already enqueues there for Meetings that were never
diarized. Counting the whole backlog would report that catch-up as re-run
progress, and cancelling would delete it. A Meeting promoted to `Front`
because somebody asked for it stops being the re-run's to cancel.

**The first start records the identity and asks for nothing.** An absent row
means this History has never recorded an identity — which is every History
today — so reading it as a model change would re-run all of History on the
first ordinary update. A transition that genuinely needs the walk calls
`begin`, which consults no row and therefore works on an installation with no
prior metadata. `begin` and `cancel` are each one transaction.

Both tables live in `schema::PENDING_MODEL_CHANGE_RERUN`, **unregistered**, so
every function fails on a current History by design.
`the_pending_rerun_is_not_registered` is the gate, beside 05's.

### There is no walker to build

`Core::run_diarization_queue` (ticket 08) already is one, and the re-run's
"walk" is `begin` putting Meetings at `Back` and letting it drain them. It
supplies, unchanged:

- **the walk** — one worker, so at-most-one-run is a property of the shape;
  `peek` orders by priority then `enqueued_at`, which is the oldest-first
  order `begin` enqueues in;
- **resume across restarts** — the queue is in the record and a Meeting
  leaves the line only once its run is over, so a Core killed mid-run comes
  back owing it;
- **wake and idle** — a `Notify` plus a 30-second timer, so a backlog
  enqueued by anything is picked up without a poll loop.

Building a second runner for the re-run would duplicate all of it and
reintroduce the one-at-a-time question the single worker answers.

### The remaining seams, smallest first

1. **Pause while a Meeting records — missing, and it is not only the
   re-run's.** `run_diarization_queue` never consults `is_recording()`; the
   only callers are `detect::policy` and `detect::driver`. So today's
   overnight catch-up already competes with a live recording, and the re-run
   would too. The fix is a guard in the worker's select, which changes
   behaviour for *all* `Back` work — its own decision, not part of
   activating the re-run.
2. **`state` errors on an unregistered schema**, with `no such table:
   diarize_rerun`. Any production reader needs a policy for that: tolerate
   the missing table as "no re-run", or read it only after activation.
   Worth settling before a reader is written, not inside one.
3. **`diarize/status` gains an optional `rerun` block** — additive
   (ADR-0028), needing a test that a Core with no re-run encodes
   byte-identically to today.
4. **`diarize/rerunCancel`** — an additive method over `rerun::cancel`.
5. **The trigger** — `begin_if_the_model_changed` at start, and `begin` from
   the wipe. Activation, so it waits on the model decision and on 05.
6. **The seeding path** — consume `claims` before `reconcile::apply` and
   re-embed each claimed Speaker's own ranges. The largest piece, it needs
   the models, and it is where the negative half returns.

## Two traps in the old branch's version, checked against this code

Both would land silently. Written here because they are the kind of thing a
rewrite loses.

**`begin_if_the_model_changed` enqueues all of History on a first start.** It
returns `Ok(None)` only when the stored `diarize_rerun` row *matches* the
current model. When there is no row at all — a History that predates the
feature, which is every History today — it falls through and enqueues every
audio-bearing Meeting. Wiring that trigger into the current build, where the
model has not changed, would start a multi-hour re-run of everything on the
next Core start. **Absent metadata is not evidence of a model change.** The
first start after the feature lands has to record the current identity and
enqueue nothing.

**`relearnable` includes the Operator, and `claims` must not.** It selects
`forgotten = 0 AND (display_name IS NOT NULL OR is_operator = 1)`, which is
right for its own purpose — the Operator is a Speaker a re-run gives a
Voiceprint back to. But this ticket says the Operator is rebuilt **by the
three channel rules alone (ADR-0029 as amended), never from the previous
model's attributions**, and those attributions are exactly what `claims`
reads. So `claims` has to exclude the Operator explicitly and leave the
channel rules responsible for it. Using `relearnable` unfiltered would seed
the Operator from the old model's guesses about which voice was theirs, which
is the one thing ADR-0029 as amended was rewritten to stop.

## Two defects the branch's tests caught, worth not re-introducing

- Cancelling a re-run stopped at 1 of 3 reported **3 of 3**: done is total minus
  remaining, and cancelling empties the queue. An `abandoned` count is the fix.
- A test computed the History it was about as `MIGRATIONS.len() - 1`, so a new
  migration silently moved it onto the index it was testing. Pin the index.
