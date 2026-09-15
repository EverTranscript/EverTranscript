# 05: A model change clears Voiceprints and keeps the record

**Parent:** `docs/prd.md` (Speakers & diarization, stories 33b-33i) and ADR-0037.

**What to build:** The migration, landing before the model it exists for, so the swap is a
registry change rather than a cliff.

When the embedding changes, old and new vectors cannot be compared — a cosine between 256 and
192 dimensions is not a smaller number, it is a meaningless one. ADR-0037 chose the wipe over
re-embedding the old exemplars' stored sample offsets, because those offsets are the old
model's choice of cuts where the Operator's naming is a statement about a whole cluster.

The migration removes every Voiceprint and every exemplar and keeps everything the Operator
would notice losing: every Speaker row, every name, the Operator flag, every segment
attribution, every correction hint. A named Speaker with no Voiceprint is an ordinary state
afterwards, and the Voice Registry says why it has none rather than showing an unexplained
empty row.

It is a versioned migration in the existing append-only sequence, and it is tested over a
file-backed database, because the claim is about what the next Core opens rather than about
in-memory state.

**Blocked by:** 04.

**Status:** ready-for-agent

- [ ] A History carrying old-model Voiceprints migrates with every Speaker, name, flag, attribution and correction intact and no exemplar left
- [ ] Tested over a file-backed database, closed and reopened
- [ ] The Registry states the reason a named Speaker holds no Voiceprint
- [ ] Migrations stay idempotent and the schema version advances by one
- [ ] Mirrors are byte-identical across the migration, since nothing an Operator reads has changed
