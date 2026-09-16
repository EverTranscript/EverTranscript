# 12: A model change re-runs History

**Parent:** `docs/prd.md` (Speakers & diarization, stories 33b-33i) and ADR-0037.

**What to build:** The last piece, and the first thing this product does that touches all of
History at once. Everything it depends on has landed, which is why it is last.

After the migration has cleared every Voiceprint, a bulk re-run walks History **oldest first**,
one Meeting at a time, and relearns what the Operator already told the product. In each
Meeting a **named, unforgotten** Speaker is seeded from its own attributed segments embedded
by the new model — corrections winning over the machine's attribution, negatives rebuilt from
corrections that took a segment away — and those windows become its new exemplars. That uses
the Operator's confirmation of a whole cluster, which ADR-0009 already puts outside the
machine's reach. Pseudonymous Speakers are re-minted and renumbered. The Operator is rebuilt
by the three channel rules alone, never from the previous model's attributions. A Meeting with
no Kept Audio cannot be re-run and keeps the attributions it has.

It is automatic, it enters the queue at the back, it pauses while any Meeting records, it
resumes across restarts, and it is cancellable. Progress lives in the Voice Registry, because
an unannounced multi-hour background job that reprocesses everything is indistinguishable,
from outside, from the product misbehaving.

**Blocked by:** 06, 08, 09, 10, 11.

**Status:** done

- [x] A named Speaker is recognized again after its Meetings are re-run, end to end from an old-model History
- [x] A forgotten Speaker is not re-embedded, and a Meeting without Kept Audio keeps its attributions
- [x] Corrections outrank the machine's attribution when seeding, and negatives are rebuilt
- [x] Recording pauses the re-run and it resumes afterwards; a just-ended Meeting is diarized ahead of the backlog
- [x] Quitting mid-run and restarting resumes rather than restarts, and reaches the same end state as an uninterrupted run
- [x] Cancelling stops it and leaves every already-processed Meeting correct
- [x] Progress, pause state and cancellation are observable through the protocol, additively (ADR-0028)
- [x] Renumbered pseudonyms and any named Speaker left without a Voiceprint are stated to the Operator rather than discovered

## What changed

Three pieces, and the first is the one that made the ticket about more than
orchestration.

**Claiming.** `cluster::claims` reads who owned each segment *before* the run
overwrites it — the previous model's attribution with the Operator's
corrections on top, which after migration 11 is the only surviving record of
who the voices are. A cluster whose segments a relearnable Speaker already
owned is handed to that Speaker outright, skipping both the resolve and the
minting floor. Seeding could not have worked: there is no vector left to seed
with, so the first Meeting of a re-run would have given a named voice to a
fresh pseudonym. The precedence is ADR-0009's — the Operator's confirmation
of a whole cluster is outside the machine's reach (DECISIONS Q137). The
negative half, "these words were not yours", is `cluster::relearn`, written
after the assignment that decided whose they were.

**The backlog.** `store::rerun` holds one row: the model identity, the
backlog's original size, and what cancelling abandoned. The model identity is
both trigger and guard, which is what makes resume-not-restart structural
rather than a flag somebody clears (Q138). The work itself stays in
`diarize_queue`, at `Back`, which already outlived the process. The worker
stands the backlog down while any Meeting records; `Front` work does not
wait.

**The telling.** `diarize/status` grows an optional `rerun` block — progress,
paused, cancelled, plus how many named voices are recognizable again, how
many still are not, and how many pseudonyms were renumbered. `diarize/rerunCancel`
is a new method rather than a new meaning for `diarize/cancel`, which names
one Meeting. Both are additive (ADR-0028), asserted by a Core-with-no-rerun
encoding byte-identically to the old shape. The CLI prints the block and
gains `diarize cancel-rerun`; no other surface reads diarize status today.

## Measured

Nine integration tests in `tests/diarize_queue.rs` and nine unit tests across
`store::rerun` and `diarize::cluster::relearning`. The whole workspace is
green apart from `detect::macos::tests::a_real_microphone_hold_is_visible_to_the_detector`,
which needs a TCC grant this machine does not have and fails identically on
`main`.

Two defects the tests caught rather than review:

- Cancelling reported a re-run stopped at 1 of 3 as **3 of 3**. Done is total
  minus remaining, and cancelling empties the queue. `abandoned` is the fix,
  and `a_cancelled_re_run_does_not_report_itself_as_finished` is the guard.
- `the_second_you_is_reduced_to_one_and_then_made_impossible` computed the
  History it was about as `MIGRATIONS.len() - 1`, so migration 15 silently
  moved it onto the very index it tests. Pinned, as its neighbours already
  were.

## Still owed

Nothing in this ticket. The re-run has never been exercised against a real
model change end to end on a populated History — the tests drive
`rerun_history_if_the_model_changed` directly and the per-Meeting half through
`claims`/`persist`/`relearn` — because doing so needs the ONNX models and an
hour of audio per Meeting. The seams either side of that are covered.
