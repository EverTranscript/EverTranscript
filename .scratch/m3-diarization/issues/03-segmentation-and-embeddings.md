# 03: Segmentation, fbank, and the two embedding models

**What to build:** The local ONNX pipeline under the seam — voice-activity and overlap segmentation, features, and the two embeddings that do two different jobs. This is the half that touches models and hardware; 04 is the half that reasons about the vectors it produces.

**Blocked by:** 01.

Status: not started

- [ ] Powerset segmentation ONNX via `ort`, on the window/step the catalog specifies, producing per-frame speaker-activity including overlap. Where a constant is taken from the catalog, cite it where it is written; where one is invented, say so
- [ ] **80-mel fbank in pure Rust.** The reference implementation does this in pure JS, so a C dependency buys nothing and costs the cross-platform build the ADR-0025 parity gate depends on
- [ ] Two embedding models, because they are two jobs (catalog M3): a cheap clustering embedding used within a Meeting, and a durable identity embedding used as the stored Voiceprint. Collapsing them makes either clustering slow or recognition brittle
- [ ] **Voiceprint span selection** is deliberate, not "whatever audio was there" (catalog M3): merge small gaps, subtract overlapped same-channel speech entirely, clip to a bounded middle span, enforce a minimum voiced duration, and take the longest few spans per speaker. A Voiceprint built from crosstalk is worse than no Voiceprint
- [ ] Models join the existing checksummed first-run download set (ADR-0034 Sanctioned Traffic unchanged — no new network path, no new host). `evertranscript models` must list, verify and fetch them like every other model
- [ ] `catch_unwind` around FFI, work in bounded windows, and a hard cap on concurrent jobs — **reject, do not queue** an overlapping diarization request (catalog M3). M1 already paid for the version of this where transcription starved capture (DECISIONS Q7); post-meeting diarization must not starve live transcription
- [ ] **It runs on Windows, and that is demonstrated rather than assumed.** M2 ended by discovering Windows detection had never worked at all while CI was green, because the code compiled and nothing called it. An `ort` load and a real inference must execute in CI on `windows-latest`, not merely link
- [ ] Degradation is honest: a missing or corrupt model leaves the Transcript unattributed and says so, and never blocks the recording or loses the Meeting
