# 09: A deleted Voiceprint stays deleted

**Parent:** `docs/prd.md` (Speakers & diarization, stories 33b-33i) and ADR-0037.

**What to build:** The forgotten mark, landing before the re-run that makes it necessary.

Deleting a Voiceprint is the product's one biometric control and ADR-0009 makes it a legible
Operator act. After a model change, a Speaker the Operator deliberately deleted looks
identical to a Speaker the migration cleared: both have a name and no vector. A re-run that
relearns named Speakers from their attributed segments would bring the deleted voice back,
and the Operator would have no way to know it happened.

So deletion now also marks the Speaker **forgotten**, a mark only that act sets. A forgotten
Speaker keeps its name and every appearance, is never re-embedded by any re-run, and is
visibly forgotten in the Registry rather than merely vector-less.

Landing this before the re-run rather than after is the whole point: the other order ships the
harm and fixes it later.

**Blocked by:** 05.

**Status:** done

- [x] Deleting a Voiceprint marks the Speaker forgotten; nothing else sets the mark
- [x] A forgotten Speaker keeps its name, its appearances, and its place in the record
- [x] A forgotten Speaker is never re-embedded, asserted directly rather than through the re-run, so the guarantee does not depend on 12 landing
- [x] The Registry distinguishes forgotten from merely without a Voiceprint
- [x] The existing delete-stops-recognition test still holds, Mirror bytes included

The guard is `speakers::relearnable`, which ticket 12's re-run consumes. Deliberately not
a guard on the correction path: an Operator re-attributing a segment to a forgotten
Speaker is a statement about that one Speaker, made on purpose, which is a different act
from a re-run sweeping the voice back in unasked.

Not retroactive. A Voiceprint deleted before this shipped left no record that it was
deleted rather than never taken, and marking those rows forgotten would invent an
Operator act that may never have happened.
