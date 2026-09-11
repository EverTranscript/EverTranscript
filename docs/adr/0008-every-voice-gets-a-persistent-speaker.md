# Every voice gets a persistent Speaker with a stored Voiceprint

> **Amended 2026-09-10 (a voice is something the Transcript contains):** "every voice" was implemented as "every cluster the diarizer left standing", and the first real History showed the difference — 503 Speakers across six Meetings of three to five people, 378 of them owning no transcribed word. A cluster now becomes a Speaker only if it owns at least one Transcript segment and, when no Voiceprint in History recognizes it, holds at least ten seconds of voice (`diarize::cluster::MIN_SPEAKER_MS`). A voice under either bar still exists as turns and its segments read "Unattributed"; it earns a Speaker in a Meeting where it actually talks. Recognition of a known voice has no floor. The Registry also plays each Speaker's voice back from the recording the Voiceprint was cut from — the legibility this ADR bought the storage with, applied to the one fact a label cannot carry. DECISIONS Q64.

> **Amended 2026-08-27 (naming is also confirmation):** the Operator's naming act is a learning signal, not just a label — it promotes that Speaker's Voiceprint to an Operator-confirmed tier that carries more weight in future matching (confirmed Voiceprints win ties; unconfirmed ones still match, conservatively). Pseudonyms are numbered ("Speaker 1", "Speaker 2", …).

Diarization resolves every voice in every Meeting to a persistent Speaker, named or not ("Speaker A, seen in 14 meetings"); naming retroactively labels all past appearances. Chosen eyes-open over named-only enrollment and text-only identity (both offered and re-challenged): cross-Meeting recall ("what did Alice say last month" works without ceremony) and diarization quality that improves with every Meeting outweigh the biometric footprint for a personal, local-only tool.

## Consequences

- The product stores Voiceprints — biometric identifiers — of Participants who never consented, created silently as a side effect of recording. Under the tool posture (ADR-0007) that legal exposure (BIPA/GDPR treat voiceprint *collection* as the regulated act; local storage does not launder it) sits with the Operator. The product's obligation is disclosure: the first-run briefing must explicitly cover voice profiling.
- Mandatory legibility surfaces: the Voice Registry (inspect every Speaker and Voiceprint, per-Speaker delete), visible match attribution in Transcripts, and — provisional, unratified — a clustering master switch and a purge-all-Voiceprints control.
- What "delete a Speaker" means is resolved in ADR-0009.
