# The record is immutable; the only biometric operation is Voiceprint deletion

Transcript words are never rewritten or excised; the only way content leaves History is deleting a whole Meeting. Speaker attribution is a live reference to the Speaker record, not text baked into the Transcript — so renaming a Speaker retroactively re-labels every past appearance, and "anonymize Alice" is just renaming her Speaker; no dedicated anonymize mechanism exists. The Voice Registry's one destructive operation is deleting a Voiceprint, which stops future recognition and touches nothing in the record. Speaker records themselves are permanent.

Renames propagate to the Markdown mirrors, which are therefore regenerable projections of SQLite (ADR-0005), not independent files.

## Considered options

A two-button "forget voice / forget + anonymize" Registry and full utterance excision were rejected: the first is subsumed by rename-propagation, the second silently rewrites what happened — a record that self-edits is the opposite of a legible guarantee.
