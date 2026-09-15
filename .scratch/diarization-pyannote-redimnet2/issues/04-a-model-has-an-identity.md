# 04: A model has an identity, and seeding honours it

**Parent:** `docs/prd.md` (Speakers & diarization, stories 33b-33i) and ADR-0037.

**What to build:** The guard that makes every later ticket safe, landing while one model is
still in use and changing nothing an Operator sees.

Two gaps, both prefactors. The pipeline stamps each embedding with a model name and version
**hardcoded in the diarizer's constructor**, so the record's account of what produced a
Voiceprint is a literal that nobody updates when the model changes. And nothing anywhere
compares that name before using a vector: the seed read returns every Voiceprint in History
regardless of which model made it, and the similarity function guards only on vector length,
so two models of the same width would be silently mixed.

Afterwards the registry states a model's identity and version, the pipeline stamps what the
registry says, and seeding refuses anything another model produced. ADR-0035 already records
the model beside every Voiceprint; this makes the column mean something.

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [ ] Registry entries carry a version, and the diarizer takes its identity from the registry rather than a constant
- [ ] A Voiceprint or exemplar from another model is never compared, asserted at the store with two models present
- [ ] Same-width vectors from different models are refused, which is the case length alone cannot catch
- [ ] No behaviour change with one model in use: the existing clustering and recognition tests pass untouched
