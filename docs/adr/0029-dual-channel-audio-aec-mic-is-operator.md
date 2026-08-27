# Audio is dual-channel with echo cancellation; the mic channel is the Operator

> **Amended 2026-08-27 (conference-room correction):** Diarization runs on *both* channels. The mic-channel claim softens from "is the Operator" to "is where the Operator is": the Operator gets an automatically created persistent Speaker (displayed "You"), matched on the mic channel by Voiceprint — bootstrapped from the dominant mic voice over the first Meetings — and any other mic-channel voices (a shared room) cluster as ordinary Speakers. The channel prior survives as a strong attribution hint in the solo case, not an axiom that falsifies a shared-room record.

Microphone and system audio are captured, processed, and persisted as two distinct channels end-to-end, with acoustic echo cancellation (DTLN-shape ONNX models via `ort`, the system channel as the echo reference) applied to the mic channel **from M1**. Diarization becomes channel-aware, which yields the strongest attribution anchor for free: **the mic channel is the Operator by construction** — never clustered against Participants — and Speakers cluster only on the system channel. Without AEC, any speakerphone meeting leaks far-end voices into the mic channel: remote speech transcribes twice, the channel prior breaks, and Voiceprint clustering is poisoned — failures users blame on "the AI." Channels persist as processed (post-AEC); raw pre-AEC audio is never kept.

Prior art: anarlog ships exactly this (embedded two-stage DTLN-style AEC ONNX, dual-track recording) on the same `ort` runtime our Diarization already requires; Meetily ships noise suppression but no AEC and carries the corresponding audio-quality complaint pile.

## Consequences

- The AEC models join the first-run model downloads alongside Whisper and the Diarization ONNX pair.
- Kept audio doubles its channel count; the storage format is ADR-0032.
- The AudioSource test seam feeds dual-channel fixtures, and echo-contaminated fixture audio becomes a required test case.
