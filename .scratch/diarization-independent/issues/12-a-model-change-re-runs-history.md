# 12: A model change re-runs History

Rewritten 2026-09-16 against `main`. The ticket of the same number on
`diarization-pyannote-redimnet2` is marked done there and did not land here;
what follows replaces it. **The policy is unchanged** — only the starting state
it is written against has moved.

**Parent:** `docs/prd.md` (Speakers & diarization, stories 33b–33i) and
ADR-0037.

**Blocked by:** 05, and nothing else. The branch listed 06, 08, 09, 10 and 11;
08–11 have all landed here, and 06 gates *activation* rather than the work.

**Status:** groundwork implemented, activation outstanding. The backlog and
its state, the queue worker's pause and ordering, the protocol surface, the
Registry block, and the bounded seeding writer (`diarize::reseed`) are all
written and tested. What is not done is activation: nothing calls the writer,
`begin` and `begin_if_the_model_changed` are unreachable, and the schemas stay
unregistered — see *Activation* below.

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

**Three of those seams have since been written** (re-checked 2026-09-16), one
of them wired and working.

**The queue worker stands bulk work down for a recording, and keeps it owed.**
`Back` work does not start while a Meeting records and stops if one starts
mid-pass, through the same cancellation `diarize/cancel` uses; the queue row
survives the pause and a restart, and the worker picks it up when recording
ends. `Front` work does not yield. This is live in `run_diarization_queue`
rather than groundwork, because the existing catch-up path already needed it:
`finish_interrupted_diarization` queues at `Back` on every start, so a Core
launched during a call was competing with it for the machine before ticket 12
existed. See `DiarizeOutcome`, `yields_to_recording`, `stand_down_for_recording`
and `Core::finish_run` in `server.rs`.

Two properties of the stop are worth carrying forward, because both were
briefly got wrong. **The stop is honoured at the persistence boundary, not
only inside the run.** `LiveDiarizer::observe` polls the token at window starts
and `diarize` returns `Ok` after its final progress tick, so a stop arriving
during the last window, the clustering pass or that tick has a successful
`Diarization` in front of it — and the transaction adopts Voiceprints, mints
Speakers, moves attributions and marks the Meeting diarized. `finish_run` reads the token again inside the
store's writer closure — `Store::write` queues onto a single writer thread and
waits, so a check on the calling side precedes an unbounded wait, and a
recording can start while the closure is still in the queue. **And the reason a run stopped is
recorded when it stops, never inferred afterwards.** A recording can start and
end inside one pass, so asking whether one is running by the time the run
unwinds reports an Operator cancellation that never happened and loses the fact
that the Meeting is still owed.

Cooperative boundaries, for anyone extending this: before the run; before
decode; before the stale rebuild; each window start; each progress tick; and
inside the store's writer closure, before the transaction is opened. Model
load, decode, the rebuild, clustering and the writer's own queue all lie
between consecutive checks, so **no single stage bounds the delay** — the
guarantee is that a run stopped before that last check writes nothing, not that
a stop lands within a window or interrupts a commit already under way.

`store::rerun` exists — `Rerun`, `state`, `begin`, `begin_if_the_model_changed`,
`cancel`, `give_up`, `is_bulk_work` — with its tables out of `MIGRATIONS`. `state` now
answers "no re-run" for an uninstalled schema, asked of `sqlite_master` by name
rather than inferred from an error string, so a genuinely broken read is still
an error; `begin` and `begin_if_the_model_changed` remain unreachable from
production, so no code path can start a backlog. `cluster::Claims` and
`cluster::claims` exist too, reading the standing attribution before
`reconcile::apply` overwrites it, and abstaining where a cluster's segments do
not all belong to one eligible Speaker. Nothing calls either.

**Still to build, and absent from `main`:**

1. **The seeding writer, which is not a restored `relearn`.** One was built
   and then **withdrawn** (Q165, `8dd6781`), and the reason bounds what
   replaces it. Its positive half would have enrolled a cluster's centroid,
   which `cluster::centroid` builds over every grouped `Observation` while
   reconciliation runs afterwards — so a claimed cluster's vector can carry
   speech no transcript segment covers, and the parts of each observation
   window lying outside the segments over it. A unanimous claim is therefore
   **not permission to enrol the raw centroid**, and that limit is written on
   `claims` itself. Its negative half was worse in a second, independent way:
   an existence check on the vector dedupes an identical retry but does not
   replace changed evidence, so correcting away from a Speaker, relearning,
   correcting back and relearning leaves the stale negative standing, and a
   repartition files a second vector beside the obsolete one.

   What is actually owed is **range-bounded re-embedding, positive and
   negative, with replacement scoped to the source that produced each piece of
   evidence** — a vector cut from exactly the claimed ranges, carrying a stable
   identity for the correction or cluster it came from, so writing it again
   supersedes rather than accumulates. Neither the embedding API nor the
   exemplar table carries that today. Writing `Claims::denials` into centroids
   would rebuild the withdrawn writer; so would handing `Claims` to
   `reconcile::apply` and enrolling what it assigns.
2. ~~The wiring~~ — **done** (Q215, reordered by Q217; trigger in Q221).
   `diarize_meeting` reads the plan and embeds its ranges with the run's own
   embedder, `finish_run` commits them inside the same transaction as the
   attribution and the queue row, and `Core::rerun_if_the_model_changed` runs
   at every start. The trigger records the current identity and enqueues
   nothing on its first start, since a History with no `diarize_rerun` row is
   every History today and absent metadata is not evidence of a model change.
   It is still unreachable in the field: the gate now lives inside
   `begin_if_the_model_changed`, which answers `None` rather than failing when
   the tables are absent, so the one function a Core calls on every boot cannot
   be reached past an unregistered schema and no caller has to remember that.
3. The Registry's progress, pause and cancel affordances. Nothing in
   `clients/electron/src` reads a re-run; the existing `rerunSetup` is
   onboarding and unrelated. The wire is ready for it: `DiarizeStatusResponse`
   carries an optional `rerun` block — `total`, `done`, `remaining`,
   `abandoned`, `cancelled`, `pausedForRecording` — absent whenever no backlog
   was asked for, so a History with no tables and one whose first start merely
   recorded its embedding both encode byte-identically to the old shape
   (ADR-0028). `diarize/rerunCancel` stops only this backlog's own bulk rows
   and, when it is one of them, the Meeting being walked; a promoted `Front`
   Meeting and the catch-up pass are left alone. That eligibility is decided
   *inside* the mutation, on the writer, because a promotion can commit
   between an answer read beforehand and the mutation that uses it. An
   unreadable re-run is an error and reaches the Client as one — only an
   absent schema, an absent row and the first-start baseline are successful
   absences, and stopping any of those three is a no-op.

## Built: the caller (in `diarize_meeting` / `finish_run`)

**The seeding commits inside the run's own attribution transaction, before
`persist`.** `reseed::plan` reads `transcript_segments.speaker_id` with the
newest correction on top — the attribution the *previous* model left — and
`commit` revalidates that plan before replacing anything, refusing as
`Refused::Moved` if the record has moved underneath it. The only statement that
moves it is `reconcile::apply`, which runs near the **end** of `finish_run`'s
transaction: `run_guarded` and `LiveDiarizer` return a `Diarization` and write
nothing, and `reconcile::reconcile` is a pure mapping. So the previous
attribution is still intact throughout the run and for most of the transaction,
and the commit has a window inside it where revalidation is meaningful and the
freshly rebuilt seeds are already visible to the matching that happens in the
same transaction. There is no forced choice between committing early and being
refused for ever.

The work is therefore split by cost, not by transaction:

- **Before the run, outside any transaction.** The cancellable job is registered
  first, then `rerun::is_bulk_work` is checked and `reseed::plan` read beside
  the stale-exemplar read. Registering first is what makes the expensive part
  interruptible.
- **During the run, outside any transaction.** `reseed::embed_ranges` cuts and
  embeds the planned ranges with the run's **own loaded embedder**, so a re-run
  loads one model rather than two. Minutes of model time, none of it holding
  History's single writer.
- **Inside `finish_run`'s writer closure.** Bulk membership is re-checked at the
  commit boundary, `cluster::claims` is read while the old attribution still
  stands, `reseed::commit` replaces the evidence, and then `persist`,
  `reconcile::apply`, `attach_operator` and `diarize_queue::finish` complete.
  One `transaction.commit()` covers all of it.

A cancel, a stop, or a failure anywhere after the plan therefore leaves **no**
partially re-seeded evidence: the whole closure rolls back together. The
embedder is loaded only when there is a range to embed — a plan with no ranges
still has to commit, because a Speaker whose every segment was corrected away is
in `owners` precisely so its Voiceprint is recomputed without any.

**Persistence had to be taught about the rebuilt evidence.** `persist_with`
deletes this Meeting's machine exemplars for each resolved Speaker and installs
the whole-cluster centroid in their place — a vector `live::assemble` built over
every grouped `Observation`, before reconciliation mapped segments to clusters,
so it carries speech no transcript segment covers. Left alone it would erase the
bounded exemplars just rebuilt and enrol unvouched audio over them.
`cluster::Rebuilt` threads two sets through the existing lifecycle:

- `reseeded` — Speakers whose bounded rows were just written. Their machine
  exemplars are neither deleted nor replaced; the resolve loop assigns and moves
  on.
- `claimed` — clusters whose segments unanimously name an eligible Speaker, from
  `cluster::claims`. A claim is **assignment authority only**: it says who those
  segments belong to, never that the rest of the cluster's vector is theirs, so
  a claimed cluster is assigned and never enrolled.

Ranges are rebuilt from the attribution independently of claims, so a named
Speaker's ranges inside a mixed cluster survive even though the cluster carries
no unanimous claim. An empty `Rebuilt` — every path that is not a bulk re-run —
leaves the lifecycle exactly as it was. The Operator's channel rules and the
forgotten/pseudonym exclusions are untouched, being upstream of all of this.

**An unrecovered Meeting stays owed, and stays owed.** A `Refused::Moved` plan
or a failed embedding returns `DiarizeOutcome::Owed`: nothing written, the queue
row kept. There is **no attempt budget** — an earlier version gave up after two
passes, and a count of failed passes is not evidence that a Meeting is
unprocessable, since two `Moved` refusals are most likely two Operator
corrections landing while the model ran. What bounds it is a rate: the worker
waits on the same wake-or-thirty-seconds select a pause uses before coming back
to it, with its own reason. What ends it is the explicit stop the Operator
already has.

An unexpected `Err` out of `diarize_meeting` is owed too, not skipped. A
transaction that fails at the end of `finish_run` has rolled the attribution,
the rebuilt evidence and the queue removal back together; converting that to
`Skipped` deleted the work a few lines later, which is silent loss on the path
that understands least about what went wrong. The cost of keeping it is
head-of-line blocking, so anything genuinely unprocessable now says so itself:
no audio, no models, a queue row whose Meeting is gone, and `Refused::Gone` —
which is `Skipped` rather than `Owed` because no later pass grows a recording
back. What is not allowed is logging a refusal and marking the Meeting
completed, which would report a partial walk as a successful one.

**The gate is structural.** `rerun::is_bulk_work` is `installed() AND a row in
diarize_rerun_backlog`, extracted from the copy `give_up` was already computing
so the two cannot drift. The tables are not in `MIGRATIONS`, so it answers
`false` on every History in the field and the path is unreachable there — pinned
by `a_history_in_the_field_is_never_reseeded`, which offers a Meeting with Kept
Audio and a named Speaker with evidence in it and asserts nothing ran, with an
empty models directory so that getting past the gate would fail loudly rather
than quietly. A Front member promoted into the bulk backlog is still eligible;
the gate is about the schema, not about how the row arrived.

## Acceptance criteria

- [ ] A named Speaker is recognized again after its Meetings are re-run, end to
      end from an old-model History — the wiring is there and asserted through
      the resolver (`a_wipe_then_a_bounded_rebuild_leaves_the_named_voice_matchable_again`:
      wipe, rebuild, `cluster::resolve` names Alice again) and through the real
      completion path (`rebuilt_ranges_survive_the_run_that_would_have_replaced_them`,
      `an_unanimous_claim_attributes_without_enrolling_the_cluster`), but with
      synthetic embeddings. Whether a real model's vectors recognize the same
      person is measurement, and waits on the model decision and on the trigger
- [x] Re-seeding and the run commit together or not at all — a stop before the
      writer and a failure after `reseed::commit` both leave the previous
      evidence exactly as it was
      (`a_stop_before_the_transaction_leaves_no_rebuilt_evidence_behind`,
      `a_failure_after_reseeding_leaves_no_rebuilt_evidence_behind`), and a plan
      that moved during inference returns `Owed` with the queue row kept
      (`a_correction_during_inference_keeps_the_meeting_owed_and_writes_nothing`)
- [x] Seeding comes from the attributed ranges, not from the old model's stored
      sample offsets — asserted in
      `reseed::tests::the_model_is_handed_only_the_audio_the_ranges_cover_in_both_directions`,
      which records every stretch the reader is asked for. Not from whole
      clusters as the ticket first said, per Q205: `claims` answers unanimity
      over a cluster, and gating on it would discard a named Speaker's own
      usable ranges whenever the cluster around them came out mixed
- [x] A forgotten Speaker is not re-embedded, and a Meeting without Kept Audio
      keeps its attributions — `plan` returns `None` without kept audio, and a
      Speaker forgotten while embedding is one of the three disturbances in
      `a_record_that_moved_while_embedding_refuses_the_vectors`
- [x] Corrections outrank the machine's attribution when seeding, and negatives
      are rebuilt — driven through `speakers::correct_attribution` rather than
      fabricated hints (Q208). What a rebuilt negative does is stated exactly
      rather than overclaimed: it is not a repellent, and the vector being
      cleared is what withdraws recognition (Q209)
- [x] Recording pauses the re-run and it resumes afterwards; a just-ended
      Meeting is diarized ahead of the backlog — in the queue worker, covered
      by `tests/diarize_queue.rs` (five cases, with a no-recording control) and
      by `server::tests::a_stop_that_arrives_after_a_successful_pass_writes_nothing_and_stays_owed`,
      which drives the real persistence transaction with a synthetic successful
      run and needs no models
- [x] Quitting mid-run and restarting resumes rather than restarts, and reaches
      the same end state as an uninterrupted run —
      `a_backlog_interrupted_by_a_restart_ends_where_an_uninterrupted_one_does`
      builds the same backlog twice, drops the Core after one Meeting, restarts
      through the real trigger and compares the queue, the re-run block and the
      evidence counts against the run nobody interrupted
- [x] Cancelling stops it, reports honestly how far it got, and leaves every
      already-processed Meeting correct —
      `cancelling_the_backlog_keeps_what_it_walked_and_counts_the_rest_given_up`
      asserts `(total 3, done 1, remaining 0, abandoned 2, cancelled)` through
      `diarize_status`, that the walked Meeting stays diarized, and that a later
      start does not resurrect the backlog
- [x] Progress, pause state and cancellation are observable through the
      protocol, additively (ADR-0028), and drawn in the Registry
- [x] Renumbered pseudonyms and any named Speaker left without a Voiceprint are
      stated to the Operator rather than discovered

## Activation

Same two dependencies as 05, plus one of its own:

1. The model choice (ticket 06) is settled by the user. Its measurement is
   complete: the embedding A/B ran on dev and held-out test, and the split-model
   question the user released for measurement has been measured too (Q180,
   Q183), so neither is an open experiment. What is still the user's is the
   adoption — which model and configuration to run, and which recognition
   outcome to prioritise — together with the ≥ 2.0-point DER bar, which they
   asked for and deliberately left undecided.
2. Ticket 05 has landed, since this re-runs into the state its wipe leaves.
3. **A real end-to-end exercise needs the ONNX models and roughly an hour of
   audio per Meeting**, which is why the branch's version shipped with that
   admittedly untested. The seams either side are driven directly, and the
   queue-and-arithmetic half is now driven through the real completion path
   offline (Q222) — an interrupted backlog is compared whole against an
   uninterrupted one, and a cancelled one against what the Client is told. What
   stays uncovered is whether a real model's vectors recognize the same people
   afterwards, which is measurement, not wiring, and waits on (1).

No part of this may be run against a real History before (1) and (2). Writing
and testing the trigger is safe and is done; **registering the schema is what
makes it fire**, and that stays out of `MIGRATIONS`.

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

The negative half came back with the seeding path, which re-embeds the
claimed ranges and had both of the missing inputs in hand: a vector bounded to
what the correction actually covered, and a stable source identity —
`(speaker_id, meeting_id)` — to scope an atomic replacement to. Both defects
above are covered by `reseed`'s away-then-back regression, which drives
`speakers::correct_attribution` rather than fabricating the hints.

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

1. ~~Pause while a Meeting records~~ — **done** (Q187, Q190, Q192). The
   worker yields only `Back` work, keeps the row owed, and honours the stop
   inside the store's writer closure. It changed behaviour for all `Back`
   work, including today's catch-up pass, which was the point.
2. ~~`state` errors on an unregistered schema~~ — **done**. It answers "no
   re-run" for an absent table and propagates every other failure.
3. ~~`diarize/status` gains an optional `rerun` block~~ — **done**, with the
   byte-identical test for both the no-tables and the first-start-baseline
   cases.
4. ~~`diarize/rerunCancel`~~ — **done**, over `rerun::cancel`. The
   peek-to-registration window is closed too, and without a lock across
   inference: registration and the check that the work is still queued are
   one step under the job lock, which a stop must also take, so either the
   job is registered and the stop finds its handle or the row is gone and the
   run returns before claiming anything.

   **Five things take a Meeting out of the queue, and `done` does not claim
   they are the same.** A committed run (walked); a run that never reached a
   transaction — no Kept Audio, no models, a failure on its own recording
   (nothing to rebuild, and nothing would be next pass); the Operator
   cancelling one Meeting; `rerun::cancel` stopping the backlog; and the
   Meeting being deleted, which cascades the queue row and the membership
   away together. The two deliberate give-ups are counted in `abandoned` and
   subtracted, so `done` means **processed or no longer processable** — not
   "Voiceprints rebuilt" — and that is written on `Rerun::done` rather than
   inferred. A deleted Meeting lands in `done` because nobody stopped it; a
   trigger on `meetings` to count it separately would be more machinery than
   the distinction earns.

   **A run takes its own queue row out inside the transaction that writes the
   attribution.** That is what makes a stop unable to miscount a walked
   Meeting as abandoned — there is no window in which one still looks owed —
   and it replaced an attempt to tell them apart by comparing `diarized_at`
   with the re-run's start, which cannot work: `julianday` rounds two stamps a
   ten-thousandth of a second apart to the same value, and a wall clock can be
   adjusted under them either way. `DiarizeOutcome::Skipped` is the outcome
   for a run that never reached a transaction — no audio, no models, a failure
   — and is now the only one the worker removes a row for, since removing one
   after a commit could delete a fresh request made in between.

   **Membership hangs off the queue row, not off the Meeting.** That is what
   makes retirement part of whichever transaction removed the row, including
   the attribution commit, with nothing in the hot path to remember it. Hung
   off `meetings` it outlived the work: a Meeting the backlog had walked,
   queued again by hand, joined back onto the old membership and was counted
   still owed; cancelling that fresh request raised `abandoned` for a walk
   that had happened, and a bulk stop would have deleted a request the re-run
   never made. A promoted `Front` row keeps its queue row and so keeps its
   membership, which is what keeps promotion out of `done`.
5. ~~The Registry reports it~~ — **done** (Q203). An optional block above the
   Speaker list, drawn only when `rerun` is present, polled while the screen
   is open and stopped when it closes. It **reports and stops; it starts
   nothing** — no begin, no endpoint for one. `done` is drawn as "gone
   through" rather than as a result, `abandoned` stays its own count, and a
   stopped re-run that still has promoted work says what that row is instead
   of reading as finished. The pseudonym renumbering and the named Speaker
   left without a Voiceprint are stated there, the latter in the same words
   the row itself uses.
6. **The trigger** — `begin_if_the_model_changed` at start, and `begin` from
   the wipe. Activation, so it waits on the model decision and on 05.
7. ~~The seeding writer~~ — **written** (Q204, Q205), in `diarize::reseed`,
   called by nothing. `plan` reads a Meeting's ranges, `embed_ranges` embeds
   exactly those through an injected reader and model, and `commit` replaces
   what the Meeting says and recomputes the Voiceprints that moved.

   **Not off `claims`.** Claims is a shortcut for assigning a whole cluster;
   a named Speaker's own usable ranges are still theirs when the cluster
   around them comes out mixed, so the ranges come from the segments. Not
   from cluster centroids either — `claims`'s own doc says why — and not from
   the old model's saved sample cuts (ADR-0037).

   **The replacement owns `(speaker_id, meeting_id)`, every writer's rows.**
   Both columns already exist, so no new provenance and no new migration.
   Narrower would not have worked: `speakers::correct_attribution` writes
   against the same Meeting, and a correction that moved a segment away and
   then back leaves a negative behind — replacing only this path's rows would
   re-derive the positive and leave that negative standing.

   **What a rebuilt negative does, exactly.** It is retained and rebuilt as
   evidence, and that is all: current matching ignores it. `centroid` filters
   negatives out, `sample_source` filters them out, and `seeds` reads the
   positive Voiceprint column rather than the rows — so nothing scores
   against one. Nor does the stored negative drive the rebuild: `plan`
   derives both signs from the latest hint in `attribution_hints`, not from
   what is already in `speaker_exemplars`. What withdraws recognition is the
   vector going — which `commit` does through `clear_voiceprint`, never
   `delete_voiceprint`, so a recomputation cannot leave a Speaker marked as
   one the Operator forgot.

   ~~What is left for activation is the caller~~ — **written** (Q215,
   reordered by Q217), in `diarize_meeting` and `finish_run` behind
   `rerun::is_bulk_work`. The split across the write transaction is the part
   it had to get right. `plan` reads, and `embed_ranges`
   decodes audio and runs the model — minutes of work per Meeting, and both
   belong **outside** any transaction, or a re-run holds a write lock over
   History for as long as it takes to embed. Only `commit` runs inside one —
   the run's own attribution transaction, where the previous attribution is
   still intact because `reconcile::apply` has not run yet. It
   re-reads the plan, compares it to the one the vectors were computed
   from, and refuses (`Refused::Moved`) if anything moved while embedding.
   Revalidation and replacement are the transactional part; reading and
   embedding are not. Whether a real model's vectors recognize the same
   person again is measurement, not a test, and waits on the model decision.

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

**`relearnable` includes the Operator, and `claims` must not.** Already
handled — `claims` filters the Operator out and has tests for it, and
`reseed::plan` uses the same filter. Kept here because it is why both do.
It selects
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
