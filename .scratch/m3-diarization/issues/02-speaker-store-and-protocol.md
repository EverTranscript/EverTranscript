# 02: Speaker storage, exemplars, correction hints, and the protocol surface

**What to build:** The record's side of Diarization — what a Speaker is on disk, what a Voiceprint is, how a correction is stored without rewriting anything, and how a Client learns about all of it.

**Blocked by:** nothing (can land beside 01).

Status: done

- [x] Audited. The verdict, recorded in migration 7 itself: the shape was sound and the *cardinality* was wrong. One `voiceprint` BLOB per Speaker cannot represent a voice across a headset, a laptop mic and a conference phone, and ADR-0008 promises recognition that improves with every Meeting — which a single overwritten vector cannot do. The column stays as the current best identity vector; the observations behind it became rows. Original criterion: It has existed since M1, has never been written to, and was designed by a session that had not built Diarization — treat it as a proposal, not settled schema. Migrations are additive and append-only, as every prior migration in `schema.rs` is
- [x] **Exemplars**: a Speaker accumulates embedding exemplars over Meetings, not just one vector. One vector per Speaker cannot represent a voice across a headset, a laptop mic and a conference phone, and ADR-0008's promise is recognition that *improves* with every Meeting
- [x] Rows carry model and model version (already columned), so a model upgrade re-embeds cleanly from kept audio rather than silently comparing vectors from two different spaces — the failure mode that makes recognition mysteriously degrade
- [x] **Correction hints as their own table** (ADR-0009 as amended): a re-assignment is an appended row, the machine's `speaker_id` on the segment is never overwritten, and the display join prefers the hint. A hint records who it was re-assigned to, who it was taken from, and when
- [x] `confirmed` is set by naming and is what makes a Voiceprint outrank an unconfirmed one when matching (ADR-0008 as amended)
- [x] Deleting a Voiceprint clears the vectors and exemplars and leaves the Speaker row, its name, and every segment reference untouched (ADR-0009, story 31). Deleting is the **only** destructive biometric operation
- [x] Protocol additions are **additive only** (ADR-0028): Speaker records, per-segment attribution, diarization job state, and the Registry read/delete surfaces. Existing requests and notifications keep their shapes; generated TypeScript bindings and JSON schemas are regenerated and committed, as `scripts/check.sh` already enforces
- [x] The Mirror projection renders Speaker names — and `render` takes them as an argument rather than looking them up, so the projection stays pure (ADR-0005): same Meeting, same segments, same names, same bytes. Unnamed Speakers are numbered by first appearance and the number is deliberately **not** stored, because a persisted "Speaker 2" reads as a name somebody chose and would be wrong the moment another Meeting numbered its voices differently. Original criterion: and a rename or a correction marks every affected Meeting dirty so Mirrors follow (ADR-0009: Mirrors are regenerable projections, never independent files)
- [x] A Meeting delete still removes everything that belongs to it and nothing that does not: its segments and hints go, Speakers and their Voiceprints survive, because Speakers are cross-Meeting records
