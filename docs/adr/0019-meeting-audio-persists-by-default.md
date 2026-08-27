# Meeting audio persists by default

> **Amended by ADR-0029/0032:** kept audio is two channels (mic + system) stored as one stereo AAC file per Meeting, encoded incrementally by a bundled ffmpeg; the size estimate below revises to ~20–30MB/hr. Everything else stands.

After transcription and Diarization complete, the compressed audio (~10–15MB/hr) stays on disk as part of the Meeting. A global keep-audio knob and whole-Meeting delete (ADR-0009) cover removal. Default-discard would be the product silently destroying data it can never recover — ASR errors and mis-clustered Speakers frozen at day-one model quality forever. The product's privacy stance is about the cloud; local artifacts on the Operator's own disk are the Operator's property.

Kept audio unlocks future "Enhance": explicit Operator-invoked re-transcription/re-diarization with better models, replacing derived records by explicit act — the product itself still never rewrites the record (ADR-0009 intact). A rolling auto-expiry window was rejected as silent timed deletion — the mechanism-shape this design rejects.
