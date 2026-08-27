# Audio is dual-channel with echo cancellation; the mic channel is the Operator

Microphone and system audio are captured, processed, and persisted as two distinct channels end-to-end, with acoustic echo cancellation (DTLN-shape ONNX models via `ort`, the system channel as the echo reference) applied to the mic channel **from M1**. Diarization becomes channel-aware, which yields the strongest attribution anchor for free: **the mic channel is the Operator by construction** — never clustered against Participants — and Speakers cluster only on the system channel. Without AEC, any speakerphone meeting leaks far-end voices into the mic channel: remote speech transcribes twice, the channel prior breaks, and Voiceprint clustering is poisoned — failures users blame on "the AI." Channels persist as processed (post-AEC); raw pre-AEC audio is never kept.

Prior art: anarlog ships exactly this (embedded two-stage DTLN-style AEC ONNX, dual-track recording) on the same `ort` runtime our Diarization already requires; Meetily ships noise suppression but no AEC and carries the corresponding audio-quality complaint pile.

## Consequences

- The AEC models join the first-run model downloads alongside Whisper and the Diarization ONNX pair.
- Kept audio doubles its channel count; the storage format is ADR-0032.
- The AudioSource test seam feeds dual-channel fixtures, and echo-contaminated fixture audio becomes a required test case.
