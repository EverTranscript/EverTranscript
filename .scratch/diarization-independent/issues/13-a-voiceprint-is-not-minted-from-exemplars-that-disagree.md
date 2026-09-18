# 13: A Voiceprint is not minted from exemplars that disagree

Raised 2026-09-17 out of ticket 12's verification, not from the original
breakdown. It is a defect found in the shipping mint path, so unlike 05 and 12
it has no counterpart on `diarization-pyannote-redimnet2`.

**Parent:** `docs/prd.md` (Speakers & diarization, stories 33b–33i) and
ADR-0037. ADR-0008 as amended is what makes it urgent rather than cosmetic: a
confirmed Voiceprint outranks an unconfirmed one when matching, so a blended
vector filed under a confirmed name wins the ties it should lose.

**Blocked by:** nothing technically. It is blocked on the user: three choices
below have to be made before there is anything to build.

**Status:** **held for user.** Named in Q246, measured in Q247, sharpened by
Q248 and Q249. Deliberately not built — every remaining question is a
judgement about what a Speaker silently loses, which is the same kind of call
as `MATCH_FLOOR` and is not the agent's to take.

## What to build

`cluster::centroid` averages a Speaker's exemplars, L2-normalizes, and tests
nothing about whether those exemplars are of the same voice. Its one mint site
is `cluster::refresh_voiceprint`, which the Q237 relearn path reaches through
`adopt_rebuilt`. A grep for coherence, spread, outlier or dispersion across
`cluster.rs` and `reseed.rs` finds nothing.

So a Speaker whose attributed segments carry two voices is handed a vector
representing neither, stamped with their name, marked with the model, and used
for recognition — and the product has no way to notice.

The guard is one check at that mint site, before `set_voiceprint`: measure
whether the exemplars agree, and on disagreement fall through to the
`clear_voiceprint` branch that already exists there for *"no vector this
Speaker's record supports"*. No new state, no new table, no new call site. A
Speaker with no Voiceprint is already an ordinary state — ticket 05 made the
Registry able to say why one is empty.

## What the measurement established

All read-only, on `macbook-pro-nickel`'s real History, replicating
`refresh_voiceprint` exactly rather than approximating it.

**It is not hypothetical, and it is not confined to one host.** On that
History the Operator's relearned Voiceprint scores **0.6761** and **0.6724**
against the two `Ming Chen` rows — above the 0.62 match floor — and **0.3473**
against the pseudonym holding the Operator's own clean voice (Q248). The
vector filed under the Operator's confirmed name matches a different
participant and fails to match the Operator. The walk log shows
`operator_rule=Discriminant(2)` firing, so it was already attributing speech
on that vector. The same mint serves every Speaker on every host; the Operator
is merely the most exposed, because `Identified::Dominant` hands them a whole
cluster.

**The cap is the mechanism, and it does not sample.** `MAX_EXEMPLARS` is 32
and `centroid` takes `.rev().take(32)` over exemplars read `ORDER BY id`,
whose ids are UUIDv7 minted at insert — so the newest 32 are the last rows
written, the contiguous tail of the last Meeting that contributed any. The
weighted centroid of those 32 reproduces the stored Voiceprint at
**1.000000**; the weighted centroid of all 483 matches it at **0.516952**. All
32 came from one Meeting, `01a08e16`, windows 4325–5009s, and **all 32 overlap
system-channel speech** (Q249). One Meeting's ending decided an identity.

**Which measure discriminates, and which one inverts** (Q247). The obvious
measure is the wrong one: `min` exemplar-vs-centroid puts the Operator at
0.5142 and the weakest named control at **0.4170**, so any floor refusing the
Operator refuses a legitimate Speaker first. **Mean pairwise cosine** is the
measure to hang a threshold on — it forms no centroid, so it is immune to the
blur that defeats centroid scoring; it is not a single-worst-pair statistic;
and it needs one threshold rather than a floor *and* a fraction.

## The three choices held for the user

Nothing here is a default an agent should pick.

1. **The threshold.** On mean pairwise cosine, the admissible band depends on
   choice 3: **(0.4426, 0.6143)** if the guard scores the capped 32 the mint
   actually consumes and must catch the failure that happened, or
   **(0.3674, 0.6143)** if it scores the Speaker's whole record. The upper
   bound is the weakest named control in both cases — the duplicate `Ming
   Chen` row, which may itself be mildly contaminated; excluding it widens the
   band considerably, and whether to exclude it is a question about that
   Speaker, not about the guard.

2. **Refuse, or keep the coherent subset.** Refusing is minimal and loses more:
   a contaminated Speaker silently stops being recognized. Subsetting mints
   from the dominant coherent group and loses less, but is no longer a
   one-line check. Q248 argues these are not symmetric — a blended Voiceprint
   is not vague, it is confidently wrong about somebody else — which is a
   reason to fail closed, not a decision to.

3. **Where the guard reads.** Scoring the capped 32 measures what the mint
   consumes; scoring the whole record measures what the Speaker's evidence
   actually looks like. On this History the whole record scores 0.3674 mean
   pairwise against the capped slice's 0.4426, so the whole record makes the
   incoherence plainer and keeps the wider band — at the cost of refusing a
   Speaker over rows the mint would never have used.

## The band after ticket 14 landed

**Q254, 2026-09-17.** Ticket 14 shipped first, so the band below was
re-measured over the exemplars the mint now actually selects — read-only,
`mode=ro`, real History:

| | pre-14 | post-14 |
|---|---|---|
| Operator (must be refused) | 0.3190 | **0.3190** |
| weakest named control (must not be) | 0.6105 | **0.5867** |
| admissible band | (0.3190, 0.6105) | **(0.3190, 0.5867)** |

The lower bound did not move: the Operator has 9 usable exemplars over 6
Meetings, under the cap, so the spread is a no-op for them. The upper bound
fell, because the weakest control — the 230-exemplar `Ming Chen` — now draws on
5 Meetings instead of 3, and breadth costs coherence. **The predeclared 0.50
remains inside the band**, with 0.181 above the Operator and 0.0867 below that
control; the headroom above the control was 0.1105 before.

Two facts about the current History that bear on choices 1 and 3 above:

- **Choice 3 is moot for the Operator on this History, and the user has
  already settled it.** The Operator's record was reduced to 9 rows by the
  Q236 relearn, so "the capped 32" and "the whole record" are now the same
  set and both score 0.3190 — the 0.4426 / 0.3674 split was measured on the
  pre-relearn 483 rows. The user's decision is the post-14 selection: one
  read, one path, no separate whole-record score.
- **The Operator currently has no Voiceprint at all.** The relearn left nine
  exemplars and no stamped vector, so the wrong Voiceprint Q246–Q249
  diagnosed is not in the database today. That is not the guard working — the
  guard does not exist yet — and it means the walk in decision (3) is
  confirming that a *re*-mint is refused, not that an existing wrong one was
  removed.

## Interaction with ticket 14

Ticket 14 changes *which* 32 exemplars the mint sees, so it moves the band
above. The weakest legitimate control on mean pairwise — the 229-exemplar
`Ming Chen` row, at 0.6143 — **is** this band's upper bound, and spreading its
32 across the four Meetings it was heard in drops it to 0.5846. A threshold
picked at, say, 0.60 would pass that Speaker today and refuse it after 14
lands. **Whichever of the two lands second must re-measure the band**, and if
both are wanted, choosing 14's selection rule first costs nothing and settles
the numbers this ticket's threshold is picked against.

## Acceptance criteria

Unticked by design: the work has not started, and criterion 1 gates the rest.

- [ ] The three choices above are made by the user
- [ ] A Speaker whose exemplars disagree by the chosen measure gets no
      Voiceprint minted, through the existing `clear_voiceprint` fall-through
      rather than a new path
- [ ] Every named Speaker on the real History that holds a legitimate
      Voiceprint today still holds one afterwards — the controls the guard
      must not refuse
- [ ] The guard is exercised against a fixture reproducing the real failure:
      a Speaker whose newest exemplars are one contiguous double-talk stretch
- [ ] The Registry distinguishes "refused for disagreement" from the three
      states ticket 05 already keeps separate, or it is stated why it need not

## Caveats on the numbers

Every figure above comes from **one** History of ten Meetings, and the
Operator's pre-fix statistics rest on 9 exemplars (36 pairs) against the
controls' 32 (496 pairs). That asymmetry cuts in the separation's favour — the
controls had thirteen times the opportunity to produce a low pair and stayed
above 0.18 — but the band is evidence that a workable threshold exists, not a
measurement of where it generalises.
