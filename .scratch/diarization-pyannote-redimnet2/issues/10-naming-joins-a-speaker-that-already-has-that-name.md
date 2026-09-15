# 10: Naming joins a Speaker that already has that name

**Parent:** `docs/prd.md` (Speakers & diarization, stories 33b-33i) and ADR-0037.

**What to build:** The act that keeps the glossary's promise true. "Naming retroactively labels
all past appearances" is false the first time a voice comes back as a new pseudonym, and after
a model change every returning voice does.

Naming a pseudonymous Speaker with a name another Speaker already holds **joins** it: segments,
corrections and exemplars move onto the existing Speaker and the pseudonymous row is swept.
The Client confirms first, naming what will merge, because combining two identities should be
a decision rather than a side effect of typing. Joining one named Speaker into another is
refused — the one act that feels irreversible stays the one explicitly asked for.

This also closes an oddity the M3 close-out recorded, where re-diarizing after a Voiceprint
deletion left a named Speaker in the Registry with zero appearances.

A separate merge control in the Registry was considered and declined: it is the same code
behind a second control.

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [ ] Naming onto an existing name produces one Speaker holding both sets of appearances, after a confirmation that states what merges
- [ ] Named-into-named is refused with a legible reason
- [ ] Corrections and exemplars follow the segments, so the joined Speaker keeps what both were taught
- [ ] Declining the confirmation leaves both Speakers untouched
- [ ] Mirrors and the Registry reflect the join, and the Transcript's own text is unchanged (ADR-0009)
