# 05: A model change clears Voiceprints and keeps the record

Rewritten 2026-09-16 against `main`. The ticket of the same number on
`diarization-pyannote-redimnet2` is marked done there and did not land here;
what follows replaces it. **The policy is unchanged** — only the starting
state it is written against has moved.

**Parent:** `docs/prd.md` (Speakers & diarization, stories 33b–33i) and
ADR-0037.

**Blocked by:** nothing. Ticket 04 has landed (`cfd1077`), which is what makes
"the model changed" a question the code can answer.

**Status:** **done — built and activated.** Registered 2026-09-17 as
`MODEL_CHANGE_WIPE`, migration 15, on the user's instruction (DECISIONS Q228),
and **run against a real History the same day** (Q234): the gate fired on first
boot on `macbook-pro-nickel`, wiped every Voiceprint and kept the record, and
`user_version` went 14 → 16. All six acceptance criteria are met. The text
below was written while it was groundwork and still says the wipe is held out
of `MIGRATIONS`; read those passages as history — `store::schema.rs:471` lists
it, the constant is no longer `PENDING_*`, and the two tests that asserted it
was unregistered were inverted into one that asserts the pairing (Q228).

## What to build

The migration, landing **before** the model it exists for, so that a swap is a
registry change rather than a cliff.

When the embedding changes, old and new vectors cannot be compared. ADR-0037
chose the wipe over re-embedding the old exemplars' stored sample offsets, for
a stated reason that still holds: **those offsets are the old model's choice of
cuts, where the Operator's naming is a statement about a whole cluster.** The
re-learning therefore belongs in ticket 12, from attributed whole clusters and
the Operator's corrections, and not here.

The migration removes every Voiceprint and every exemplar and keeps everything
the Operator would notice losing: every Speaker row, every name, the Operator
flag, every segment attribution, every correction hint. A named Speaker with no
Voiceprint is an ordinary state afterwards, and the Voice Registry says why it
has none rather than showing an unexplained empty row.

It is a versioned migration in the existing append-only sequence, and it is
tested over a file-backed database, because the claim is about what the next
Core opens rather than about in-memory state.

## What changed since the branch wrote this

Three things, none of which touches the policy.

**Main grew a different mechanism for the same event** (Q115, ADR-0035 as
amended). `store::speakers::stale_exemplars` finds every exemplar from another
space; `diarize::runner::rebuild` re-embeds each from the sample window it
kept; `cluster::adopt_rebuilt` adopts them in the next Diarization's own
transaction. It is lazy, per-Meeting, and it re-embeds **the old model's cuts**
— the thing this ADR rejected. It rebuilds evidence and never re-runs
attribution. It is not this ticket, and this ticket is not redundant to it.

So the migration has to say what happens to that path, and the answer is that
it goes quiet on its own: after the wipe there are no stale exemplars to find,
so `stale_exemplars` returns empty and `rebuild` is a no-op until ticket 12
writes new evidence. **That must be asserted, not assumed** — a migration that
left rows behind for the lazy path to resurrect would reintroduce the old
model's cuts through the back door.

**Ticket 04 landed, so the identity is now checkable.** `registry::
DIARIZE_EMBEDDING.voiceprint()` is the single source of what the current model
stamps, and the migration can be written against it instead of a constant.

**The migration index moved.** Main is at 14; the branch's wipe was its 15th.
Whatever number it takes here, `the_second_you_is_reduced_to_one_and_then_made_
impossible` and its neighbours pin their own indices and must be checked
against the new length rather than `MIGRATIONS.len() - 1`.

## Acceptance criteria

- [x] A History carrying old-model Voiceprints migrates with every Speaker,
      name, flag, attribution and correction hint intact and no exemplar left
- [x] `stale_exemplars` returns empty immediately after the migration, so the
      lazy rebuild path cannot reintroduce the old model's cuts — asserted for
      the current identity *and* for a hypothetical next one, since the point
      is that no model can find anything to re-embed
- [x] Tested over a file-backed database, closed and reopened
- [x] The Registry states the reason a named Speaker holds no Voiceprint —
      already built (`voiceprintLabel`, `App.tsx`), which is why the fix was to
      stop the wipe destroying the provenance it reads rather than to add a
      message: the wipe nulled `voiceprint_model`, so every Speaker it cleared
      read as one that was never enrolled. It now keeps the stamp, and the
      three states stay separate — deleted by the Operator, cleared by the
      model change, never enrolled
- [x] Migrations stay idempotent and the schema version advances by one —
      unchanged, because the wipe is not in `MIGRATIONS`, which
      `the_pending_wipe_is_not_registered` asserts
- [x] Every migration-index assertion still names the migration it means —
      done ahead of registration rather than after it (Q223). All fourteen
      migrations are named constants and the three upgrade-path tests call
      `before(THE_DIARIZATION_MARK)` and friends instead of `11`, `10` and `9`,
      so each says which upgrade it stands in front of rather than which
      position. `every_migration_is_distinct_so_naming_one_is_unambiguous`
      guards the helper and pins the order. Appending the wipe would never have
      moved a prefix; what this closes is an insertion anywhere else

## What was built

`schema::PENDING_MODEL_CHANGE_WIPE`, beside `MIGRATIONS` and deliberately not
in it. Two `DELETE`/`UPDATE` statements and no schema change: every exemplar
goes, every Voiceprint column is nulled, everything else is untouched by
construction rather than by restoration.

What it keeps and why is in the doc comment; the one judgement worth repeating
is **`confirmed` survives**. Naming is confirmation (ADR-0008 as amended) and
the name survives, so clearing it would leave a named Speaker unconfirmed for
a reason nothing in the Operator's experience explains — and would make ticket
12 hand it back a Voiceprint ranking below an unconfirmed one, having been
vouched for. `store::speakers::clear_voiceprint` does clear it, but that is a
recomputation whose evidence yielded nothing, which is a different event.

Three tests in `store::schema`:

- `opening_a_current_history_leaves_its_voiceprints_alone` — the control. A
  file-backed History written by the current build, closed, reopened and
  migrated: both Voiceprints still there, `stale_exemplars` empty. Without it
  the wipe test could be measuring an ordinary open.
- `the_pending_wipe_takes_every_vector_and_keeps_the_record` — the same
  fixture, wiped, closed, reopened. Name, `confirmed`, Operator flag,
  `forgotten`, the machine's attribution and the correction hint all survive;
  every exemplar and every vector is gone while the model stamp stays, and both
  queries that could act on a stamp are checked to ignore one with no vector
  behind it — `voiceprints` finds no gallery and `speakers_with_stale_voiceprint`
  finds nothing to re-embed for a hypothetical next model. `stale_exemplars` is
  empty for the current identity and for that next one. The three reasons a
  Speaker can hold no Voiceprint are asserted to stay distinguishable from the
  three fields the Registry chooses its sentence from, with a fourth Speaker in
  the fixture who was named but never enrolled. And `relearnable` still names
  the right Speakers, which is only true because the wipe kept the names and
  the mark.
- `the_pending_wipe_is_not_registered` — the activation gate as a test rather
  than a comment, since appending to `MIGRATIONS` is the whole of activating
  it and a stray paste would clear Voiceprints on the next Core start.

## What is left

- **The Registry messaging.** The protocol already carries what it needs —
  `has_voiceprint`, `forgotten` and `voiceprint_model` on the Registry's
  Speaker — so the shape is there and the *wording* is not. A named Speaker
  with no Voiceprint has to read as "waiting to be relearned after a model
  change", distinguishable from "forgotten on purpose", which `forgotten`
  already separates. Worth writing when there is a model change to write it
  about; writing it now would describe a state the product cannot reach.
- **Activation**, below.

## Activation

**This migration must not run against a real History until the model decision
is made.** Landing it while WeSpeaker remains the model would clear Voiceprints
for no swap. Two exact dependencies:

1. The model choice (ticket 06) is settled by the user. Its measurement is
   complete: the embedding A/B ran on dev and held-out test, and the split-model
   question the user released for measurement has been measured too (Q180,
   Q183), so neither is an open experiment. What is still the user's is the
   adoption — which model and configuration to run, and which recognition
   outcome to prioritise — together with the ≥ 2.0-point DER bar, which they
   asked for and deliberately left undecided.
2. Ticket 12 exists, because a wipe with no re-run is a History nobody is
   recognized in, which ADR-0037's *Considered options* already rejected.

Until both hold, the migration can be **written and tested** but is not added
to `MIGRATIONS`. That is the safe groundwork boundary: a tested migration
behind an unapplied entry costs nothing and removes the cliff later.

## A proposal for the user, not a decision taken

The lazy rebuild path answers the same event more cheaply than wipe-and-re-run:
no multi-hour background job, no renumbered pseudonyms, and a named Speaker is
recognizable again the next time its Meeting is diarized. It is worse on two
counts ADR-0037 named — it uses the old model's cuts rather than the Operator's
confirmation of a whole cluster, and it never re-runs attribution, so an
already-diarized Meeting keeps the old model's turns.

It also has an unmeasured cost: a Speaker whose exemplars have no sample window
or whose Meeting is gone loses its Voiceprint under it, and nothing has measured
how often that is. The rebuild has never been exercised against a real model
change on a populated History — only through its seams — so "recognition
survives a model change" is not currently supported.

**Whether the lazy path should replace the policy is the user's call and is not
taken here.** If it does, 05 and 12 both close and what replaces them is a
much smaller ticket: measure the window-less exemplar rate on a real History,
and decide whether attribution needs re-running at all.
