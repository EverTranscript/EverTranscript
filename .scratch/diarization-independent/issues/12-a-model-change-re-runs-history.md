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

## The next independent piece: `cluster::claims`

Buildable now, before 05 lands and without touching activation, because it
adds no table, no migration, no protocol method and nothing that runs on its
own. It is a read over the record plus a set operation, testable against a
fixture database.

`claims` reads who owned each segment **before** the run overwrites it — the
previous model's attribution with the Operator's corrections on top — and
hands a cluster whose segments a relearnable Speaker already owned to that
Speaker outright, skipping both the resolve and the minting floor. It must go
through `store::speakers::attributed_speaker`, never `speaker_id` directly, or
it reads a correction the Operator made as though it had been ignored.

Everything else in the ticket wants either 05 (the state it re-runs into),
a migration (`store::rerun`'s backlog row), or the protocol (`diarize/status`,
`diarize/rerunCancel`). Those wait.

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
