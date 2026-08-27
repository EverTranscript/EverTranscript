# The record is immutable; the only biometric operation is Voiceprint deletion

> **Amended 2026-08-27 (attribution correction hints):** Diarization errors gain an Operator correction layer. The machine's attribution is never rewritten; a re-assignment ("this segment was Alice, not John") is an **appended hint** that wins the display join and the Mirrors, feeds the correct Speaker's confirmed exemplars, and counts as negative evidence against the wrong one. Auditability and future re-diarization survive because the machine's original conclusion is preserved beneath the hint. Lands with M3.

Transcript words are never rewritten or excised; the only way content leaves History is deleting a whole Meeting. Speaker attribution is a live reference to the Speaker record, not text baked into the Transcript — so renaming a Speaker retroactively re-labels every past appearance, and "anonymize Alice" is just renaming her Speaker; no dedicated anonymize mechanism exists. The Voice Registry's one destructive operation is deleting a Voiceprint, which stops future recognition and touches nothing in the record. Speaker records themselves are permanent.

Renames propagate to the Markdown mirrors, which are therefore regenerable projections of SQLite (ADR-0005), not independent files.

## Considered options

A two-button "forget voice / forget + anonymize" Registry and full utterance excision were rejected: the first is subsumed by rename-propagation, the second silently rewrites what happened — a record that self-edits is the opposite of a legible guarantee.
