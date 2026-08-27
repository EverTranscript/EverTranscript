# Kept audio is one stereo AAC file per Meeting, encoded by a bundled ffmpeg

Each Meeting's kept audio (ADR-0019) is a single stereo AAC file — **left = mic channel, right = system channel** (the ADR-0029 split, preserved into storage) — written incrementally during recording so a crash leaves a recoverable file (Meetily-proven chunk-and-merge, or fragmented MP4). The encoder is a **bundled ffmpeg binary managed by the Core**, chosen 2026-08-27 over the recommended pure-Rust stereo Ogg/Opus route: ffmpeg costs a ~7MB+ sidecar and its LGPL-build/update surface, and buys battle-tested encoding across real-world device churn (the Bluetooth reconnect class of bugs) plus broad decode for the post-v1 Enhance/import family.

## Considered options

Stereo Ogg/Opus via a libopus binding (recommended: better speech codec per byte, crash-safe by container design, zero new sidecars — declined) and anarlog's record-WAV-then-encode (large temp files; an interruptible post-meeting encode step) were not taken.

## Consequences

- Distribution bundles ffmpeg (LGPL build); the updater covers it.
- ADR-0019's size estimate revises to roughly 20–30MB/hr for the two channels — still trivial at Auto-Record volume.
- Enhance-era re-transcription and re-diarization get cleanly separated per-channel sources forever.
