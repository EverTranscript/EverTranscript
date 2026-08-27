# Every voice gets a persistent Speaker with a stored Voiceprint

> **Amended 2026-08-27 (naming is also confirmation):** the Operator's naming act is a learning signal, not just a label — it promotes that Speaker's Voiceprint to an Operator-confirmed tier that carries more weight in future matching (confirmed Voiceprints win ties; unconfirmed ones still match, conservatively). Pseudonyms are numbered ("Speaker 1", "Speaker 2", …).

Diarization resolves every voice in every Meeting to a persistent Speaker, named or not ("Speaker A, seen in 14 meetings"); naming retroactively labels all past appearances. Chosen eyes-open over named-only enrollment and text-only identity (both offered and re-challenged): cross-Meeting recall ("what did Alice say last month" works without ceremony) and diarization quality that improves with every Meeting outweigh the biometric footprint for a personal, local-only tool.

## Consequences

- The product stores Voiceprints — biometric identifiers — of Participants who never consented, created silently as a side effect of recording. Under the tool posture (ADR-0007) that legal exposure (BIPA/GDPR treat voiceprint *collection* as the regulated act; local storage does not launder it) sits with the Operator. The product's obligation is disclosure: the first-run briefing must explicitly cover voice profiling.
- Mandatory legibility surfaces: the Voice Registry (inspect every Speaker and Voiceprint, per-Speaker delete), visible match attribution in Transcripts, and — provisional, unratified — a clustering master switch and a purge-all-Voiceprints control.
- What "delete a Speaker" means is resolved in ADR-0009.
