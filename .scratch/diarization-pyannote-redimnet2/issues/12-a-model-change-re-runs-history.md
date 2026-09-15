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

**Status:** ready-for-agent

- [ ] A named Speaker is recognized again after its Meetings are re-run, end to end from an old-model History
- [ ] A forgotten Speaker is not re-embedded, and a Meeting without Kept Audio keeps its attributions
- [ ] Corrections outrank the machine's attribution when seeding, and negatives are rebuilt
- [ ] Recording pauses the re-run and it resumes afterwards; a just-ended Meeting is diarized ahead of the backlog
- [ ] Quitting mid-run and restarting resumes rather than restarts, and reaches the same end state as an uninterrupted run
- [ ] Cancelling stops it and leaves every already-processed Meeting correct
- [ ] Progress, pause state and cancellation are observable through the protocol, additively (ADR-0028)
- [ ] Renumbered pseudonyms and any named Speaker left without a Voiceprint are stated to the Operator rather than discovered
