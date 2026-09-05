# Meeting audio persists by default

> **Amended by ADR-0029/0032:** kept audio is two channels (mic + system) stored as one stereo file per Meeting, written incrementally so a crash leaves a playable recording. Everything else stands.
>
> **Corrected 2026-09-05:** the format is MP3, encoded in the Core's own process (ADR-0032 as reversed) — not AAC via a bundled ffmpeg, which was never actually shipped. And the size estimate this amendment gave, ~20–30MB/hr, was wrong for the format it described: AAC-192k costs **86 MB/hr**, measured on a 69.4-hour recording occupying 6.0 GB. At the 128 kbps this now uses it is **58 MB/hr**. The original figure named the right budget and the wrong bitrate.

After transcription and Diarization complete, the compressed audio (~10–15MB/hr) stays on disk as part of the Meeting. A global keep-audio knob and whole-Meeting delete (ADR-0009) cover removal. Default-discard would be the product silently destroying data it can never recover — ASR errors and mis-clustered Speakers frozen at day-one model quality forever. The product's privacy stance is about the cloud; local artifacts on the Operator's own disk are the Operator's property.

Kept audio unlocks future "Enhance": explicit Operator-invoked re-transcription/re-diarization with better models, replacing derived records by explicit act — the product itself still never rewrites the record (ADR-0009 intact). A rolling auto-expiry window was rejected as silent timed deletion — the mechanism-shape this design rejects.
