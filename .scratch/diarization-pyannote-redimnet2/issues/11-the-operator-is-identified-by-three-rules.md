# 11: The Operator is identified by three rules, and there is only ever one

**Parent:** `docs/prd.md` (Speakers & diarization, stories 33b-33i) and ADR-0037.

**What to build:** "You", aligned with how both reference products identify the same voice, and
a uniqueness defect that is reachable today.

Three rules in order (ADR-0029 as amended):

1. **Isolated mic.** Headphones the only playing output and the microphone not swapped makes
   every mic-channel cluster of that Meeting the Operator, confirmed without any act. The
   capture layer records the fact per Meeting from the output transport; the echo canceller's
   idle signal is the fallback if the Windows probe proves unreliable.
2. **Dominance.** 80% of mic time, the existing margin, and at least **20 seconds** of that
   voice. The floor is new, and it is what stops a ten-second solo test enrolling anyone.
3. **Voiceprint match**, only once the Meeting holds 30 seconds of diarized speech and at least
   two speakers. Below that gate the Operator's Voiceprint is **withheld from the whole
   resolve**, not merely from the flag — the general resolve carries it among all seeds and
   would otherwise match anyway, which would make the gate decorative.

**One flagged row, forever.** The flag has no uniqueness constraint, the lookup takes the first
row it finds, and the diarize path sets the flag without clearing any other. Delete the
Operator's Voiceprint, re-run one Meeting, and the flag lands on a freshly minted row while
the lookup still returns the old one: a second "You". A bootstrap now re-attaches to the
existing row.

**Blocked by:** 03.

**Status:** ready-for-agent

- [ ] An isolated-mic Meeting identifies the Operator with no act, and the fact is recorded per Meeting at capture time on both platforms
- [ ] A shared room still refuses to call another mic-channel voice the Operator
- [ ] A ten-second solo recording enrolls nobody
- [ ] A Meeting under the speech or speaker gate does not identify the Operator by voice, and that Voiceprint is absent from the whole resolve — asserted by the other Speakers' attributions, not only by the flag
- [ ] Deleting the Operator's Voiceprint and re-running produces no second flagged row, and the schema makes a second one impossible rather than unlikely
- [ ] The existing operator tests pass or are re-expressed against the new rules
