# 14: The exemplar cap samples a Speaker rather than taking a tail

Raised 2026-09-17 out of ticket 12's verification, alongside ticket 13. The two
are independent defects in the same mint: 13 is that nothing checks whether the
exemplars agree, 14 is that the cap chooses which exemplars by recency alone.
Either can be fixed without the other.

**Parent:** `docs/prd.md` (Speakers & diarization, stories 33b–33i) and
ADR-0037.

**Blocked by:** nothing. Unlike 13, **this needs no threshold** — it is a
change of selection rule, not of policy — so it is buildable as soon as the
selection rule is chosen.

**Status:** **built and measured** (Q254, 2026-09-17). The selection rule was
chosen by the advisor, for the user: round-robin across contributing Meetings,
newest Meeting
first, newest-first within each, until `MAX_EXEMPLARS`. No new constants; the
cap stays 32. Shipped as `cluster::spread_across_meetings`, called from
`refresh_voiceprint` only — `centroid`'s signature is unchanged, because its
other production caller (`live.rs`) averages one run's observations and a
Meeting round-robin would be meaningless there.

**What the spread changed on the real History** (read-only, `mode=ro`): three
of 27 Speakers draw on more Meetings than the tail gave them — Jack Ahn 3 → 9,
Hong Li 2 → 5, Ming Chen 3 → 5. The other 24 are under the cap and untouched.
Coherence fell where breadth rose, as predicted: Jack Ahn 0.7136 → 0.6316,
Ming Chen 0.6105 → 0.5867, and Hong Li rose slightly (0.6515 → 0.6545). The
measurement below is the pre-build reading and its absolute counts are now
stale — the History has since grown to 12 Meetings.

## What to build

`cluster::centroid` takes `.rev().take(MAX_EXEMPLARS)` over exemplars the
caller read `ORDER BY id`. The ids are UUIDv7, minted at insert, so that is a
**contiguous tail** — the last rows written, which is the end of the last
Meeting that contributed any. It is not a sample of the Speaker's history and
was never chosen to be one; the cap's own test says what it is for, and it is
cost: *"A Speaker seen in two hundred Meetings must not carry two hundred
vectors into every later clustering run."*

Bounding the cost does not require taking the newest 32. Any selection of 32 is
equally cheap downstream. What the tail buys is recency — a voice that has
changed is represented by how it sounds now — and what it costs is that one
Meeting's acoustics can own an identity outright.

The change is to select 32 across the Meetings that contributed, rather than 32
from the end. The selection rule is the only open question, and it is a design
choice rather than a threshold.

## What the measurement found

Read-only, `mode=ro`, on `macbook-pro-nickel`'s real History, 2026-09-17.
Five Speakers hold more than 32 exemplars, so those are the only rows where the
cap binds at all. For each: the tail-32 the product actually mints from, a
stratified 32 (round-robin across contributing Meetings, newest-first within
each — one candidate rule, not a chosen one), and all usable rows.

| Speaker | usable | Meetings | **tail-32 spans** | spread-32 spans |
|---|---|---|---|---|
| Jack Ahn | 888 | 7 | **1** | 7 |
| Ming Chen | 229 | 4 | **2** | 4 |
| Hong Li | 152 | 4 | **1** | 4 |
| Ming Chen (dup row) | 79 | 1 | 1 | 1 |
| Marc Ammann | 58 | 2 | 2 | 2 |

**The narrowing is severe and it is the normal case, not the pathological
one.** Jack Ahn is heard in seven Meetings and his Voiceprint is decided by
one. Hong Li likewise. Each tail-32 centroid reproduces that Speaker's stored
Voiceprint at `1.000000`, which confirms these are the rows the product minted
from and not a reconstruction.

**Three findings that argue against assuming the fix is free.**

1. **On this History, spreading changes no identity.** Every centroid — tail,
   spread, and all-rows — matches nobody else in the gallery at
   `MATCH_FLOOR`. So for these five Speakers the tail is narrow but not
   *wrong*, and spreading would have prevented nothing measurable here.

2. **Spreading lowers coherence in every case.** Mean pairwise cosine, tail →
   spread: Jack Ahn 0.6994 → 0.6214, Ming Chen 0.6143 → 0.5846, Hong Li 0.6641
   → 0.6391, Marc Ammann 0.6866 → 0.6825. Breadth costs agreement, because
   different Meetings mean different rooms, microphones and distances. A wider
   sample is a more honest picture of a Speaker and a noisier one.

3. **It therefore interacts with ticket 13 and narrows that guard's band.**
   The weakest legitimate control on mean pairwise is the 229-exemplar
   `Ming Chen` at **0.6143**, which is exactly ticket 13's upper bound.
   Spreading drops that row to **0.5846**, so a guard tuned before this change
   would start refusing a legitimate Speaker after it. The band
   (0.4426, 0.6143) becomes (0.4426, 0.5846) — still non-empty, narrower by a
   third. **Whichever of 13 and 14 lands second must re-measure the band.**

**The case where it would have mattered is the Operator, and it recurs.** The
Operator's relearned Voiceprint (Q248) was minted entirely from the tail of one
Meeting, `01a08e16`, every window of which overlapped the far end speaking.
That state lived on a throwaway copy which is deleted, so it is not re-measured
here — but it is not a one-off of that copy. On the real History today the
Operator is attributed **483 segments across 6 Meetings, and the last 32 of
them fall in `01a08e16` alone.** Exemplars are minted one per attributed
segment in chronological order, so a relearn run today would concentrate on the
same Meeting again.

## The open question, which is a design choice and not a threshold

How to select 32 across Meetings. Candidates, none chosen here:

- **Round-robin across contributing Meetings, newest-first within each** — what
  was measured. Simple, gives every Meeting a voice, and over-weights a Meeting
  that contributed two segments against one that contributed four hundred.
- **Proportional to each Meeting's contribution, with a floor of one** — closer
  to the evidence, but a dominant Meeting still dominates.
- **Newest N per Meeting, capped at the most recent M Meetings** — keeps
  recency explicit rather than incidental, at the cost of two constants.
- **Keep the tail and raise `MAX_EXEMPLARS`** — the cheapest change and the
  weakest: 888 exemplars over 7 Meetings would still be tail-dominated at any
  cap the cost argument tolerates.

Whether recency should be preserved at all is the substantive part. The tail is
not merely an implementation accident — a Speaker whose voice or setup has
changed is better served by recent evidence. What is an accident is that
recency is currently absolute.

## Acceptance criteria

- [x] A selection rule is chosen — the advisor's, for the user (Q254, re-attributed by Q285)
- [x] A Speaker heard in several Meetings has a Voiceprint drawn from more than
      one of them — asserted on a fixture through the whole mint path
      (`a_voiceprint_draws_on_every_meeting_the_speaker_was_heard_in`: two
      Meetings, forty exemplars each, the stored Voiceprint above 0.6 to both
      where the tail gave exactly 0.0 to the earlier one), and re-measured on
      the real History
- [x] The downstream cost is unchanged: still at most `MAX_EXEMPLARS` vectors
      per Speaker into clustering — asserted directly
      (`the_cap_is_spread_across_meetings_rather_than_spent_on_a_tail`)
- [x] `today_the_cap_takes_a_contiguous_tail_rather_than_a_sample` in
      `diarize::cluster::tests` is updated rather than deleted — **retargeted**
      to `the_cap_is_spread_across_meetings_rather_than_spent_on_a_tail`. It
      pinned `centroid`, whose tail-take is deliberately unchanged, so it now
      asserts the selection's share instead: the test that pinned the defect
      pins the fix (Q254)
- [x] Ticket 13's band is re-measured against the new selection: **(0.3190,
      0.5867)**, narrowed from (0.3190, 0.6105) because the weakest control
      spread. The predeclared 0.50 is still inside it, with 0.0867 of headroom
      above that control

## What this does not claim

Spreading would not have prevented the Operator's wrong Voiceprint on its own.
Every window in that Meeting's tail carried double-talk, so a sample drawn
across six Meetings would have been cleaner but not clean — the contamination
is ticket 13's subject and this ticket does not substitute for it.
