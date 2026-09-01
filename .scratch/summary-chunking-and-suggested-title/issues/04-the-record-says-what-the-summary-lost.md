# 04: The record says what the Summary lost

**What to build:** A Summary built from five of six chunks stops wearing the face of a
complete one. What ticket 03 tolerates, this ticket discloses, end to end: an additive
column following the audio-notes pattern, written in the same UPDATE as the Summary so
the existing Mirror triggers fire; the Mirror renders it beside the Summary; an
additive optional field carries it to Clients (ADR-0028); the Client shows it. The
name avoids "notes" everywhere — the glossary reserves that word for Operator writing.

**Blocked by:** 03 (the loss it discloses is born there).

**Status:** done

- [x] Migration 9 adds the nullable `summary_gaps` column; existing rows read it as absent, which means "nothing was lost" — the same convention `audio_notes` uses
- [x] Written in the same UPDATE as the Summary, inside the transaction that also carries the Suggested Title. A complete run stores nothing — tested, because a disclaimer that appears on complete Summaries would stop meaning anything
- [x] The Mirror renders it **above** the Summary rather than beside it, following the capture-notes precedent: what follows is only as complete as the run that produced it, so it must not be read as complete first. Asserted against the file on disk
- [x] Carried as an optional field, `#[ts(optional)]` and `skip_serializing_if`; the schema diff adds seven lines and removes none, so a Client that predates it sees a Meeting exactly as before (ADR-0028). Bindings and schema regenerated and committed
- [x] The Client shows it above the Summary, in both locales
- [x] Nothing here is called "notes" — column, field, and catalog key all avoid it, because the glossary reserves that word for the Operator's own writing
- [x] Two tests script the loss through the injected Backend factory and observe the stored field and the Mirror on disk; the local gate is green
