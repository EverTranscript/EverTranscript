# 02: Storage spine + Meeting lifecycle (no audio yet)

**What to build:** `evertranscript record start` / `record stop` create and finalize a (still-audioless) Meeting end to end: UUIDv7 id, rows in the STRICT-schema SQLite store behind the single writer thread, a composed Mirror appearing at the top of the History folder the moment the Meeting persists, retitle renaming the file, delete removing everything, and FTS search/export working from the CLI. The History folder materializes at its default with the hidden `.data/` store and incomplete-copy detection.

**Blocked by:** 01 (scaffold + wire tier).

**Status:** done

- [x] One writer thread owns the sole connection; a small read-only pool serves queries; STRICT tables, CHECK-validated enums, UUIDv7 ids
- [x] `record start` → Meeting exists; `record stop` → Meeting finalized and Mirror written; delta-journal tables in place (populated by ticket 06)
- [x] Mirror composed per ADR-0005 as amended: frontmatter (id, date, app, duration, speakers, audio path) → Title → Summary ("None yet") → Notes ("None yet") → Transcript
- [x] Filenames `YYYY-MM-DD-<app>-<id8>.md`; retitle renames via the dirty-queue and GCs the stale name by id8; regeneration is atomic (temp + rename)
- [x] Dirty-queue outbox with generation acks + startup reconciliation drives all Mirror writes
- [x] History folder auto-created at `~/Documents/EverTranscript` (Windows: `Documents\EverTranscript`) with hidden `.data/` (dot + Windows hidden attribute); Mirrors-without-`.data` reported as an incomplete copy with recovery guidance
- [x] Whole-Meeting delete removes rows, Mirror, and audio in one act; `search` returns FTS matches; `export` prints the Mirror markdown to stdout
