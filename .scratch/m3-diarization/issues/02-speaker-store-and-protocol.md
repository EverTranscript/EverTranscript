# 02: Speaker storage, exemplars, correction hints, and the protocol surface

**What to build:** The record's side of Diarization — what a Speaker is on disk, what a Voiceprint is, how a correction is stored without rewriting anything, and how a Client learns about all of it.

**Blocked by:** nothing (can land beside 01).

Status: not started

- [ ] Audit the M1 `speakers` table against what this milestone actually needs. It has existed since M1, has never been written to, and was designed by a session that had not built Diarization — treat it as a proposal, not settled schema. Migrations are additive and append-only, as every prior migration in `schema.rs` is
- [ ] **Exemplars**: a Speaker accumulates embedding exemplars over Meetings, not just one vector. One vector per Speaker cannot represent a voice across a headset, a laptop mic and a conference phone, and ADR-0008's promise is recognition that *improves* with every Meeting
- [ ] Rows carry model and model version (already columned), so a model upgrade re-embeds cleanly from kept audio rather than silently comparing vectors from two different spaces — the failure mode that makes recognition mysteriously degrade
- [ ] **Correction hints as their own table** (ADR-0009 as amended): a re-assignment is an appended row, the machine's `speaker_id` on the segment is never overwritten, and the display join prefers the hint. A hint records who it was re-assigned to, who it was taken from, and when
- [ ] `confirmed` is set by naming and is what makes a Voiceprint outrank an unconfirmed one when matching (ADR-0008 as amended)
- [ ] Deleting a Voiceprint clears the vectors and exemplars and leaves the Speaker row, its name, and every segment reference untouched (ADR-0009, story 31). Deleting is the **only** destructive biometric operation
- [ ] Protocol additions are **additive only** (ADR-0028): Speaker records, per-segment attribution, diarization job state, and the Registry read/delete surfaces. Existing requests and notifications keep their shapes; generated TypeScript bindings and JSON schemas are regenerated and committed, as `scripts/check.sh` already enforces
- [ ] The Mirror projection renders Speaker names, and a rename or a correction marks every affected Meeting dirty so Mirrors follow (ADR-0009: Mirrors are regenerable projections, never independent files)
- [ ] A Meeting delete still removes everything that belongs to it and nothing that does not: its segments and hints go, Speakers and their Voiceprints survive, because Speakers are cross-Meeting records
