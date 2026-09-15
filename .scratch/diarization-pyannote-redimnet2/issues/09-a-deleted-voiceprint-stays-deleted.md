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

**Status:** ready-for-agent

- [ ] Deleting a Voiceprint marks the Speaker forgotten; nothing else sets the mark
- [ ] A forgotten Speaker keeps its name, its appearances, and its place in the record
- [ ] A forgotten Speaker is never re-embedded, asserted directly rather than through the re-run, so the guarantee does not depend on 12 landing
- [ ] The Registry distinguishes forgotten from merely without a Voiceprint
- [ ] The existing delete-stops-recognition test still holds, Mirror bytes included
