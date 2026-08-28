# 06: Live ASR → transcript rows + captions channel

**What to build:** Speaking during a recording produces transcript rows and live captions: whisper-rs behind the duration-adaptive Silero chunker, per-channel transcription tagged mic/system on the shared clock, every delta journaled crash-safe, snapshot-then-tail for any Client attaching mid-Meeting, and the lossy caption subscription that can never block or kill capture.

**Blocked by:** 02, 03, 04, 05.

**Status:** ready-for-agent

- [ ] Fixture audio through the seam yields transcript rows (text, channel tag, start/end on the shared clock) via the delta journal (unique sequence, CAS fold at finalize)
- [ ] Duration-adaptive chunking per the catalog's envelope (natural-pause closes between 3s and 20s, 25s hard cap with carried context)
- [ ] A test client attaching mid-recording receives the Meeting snapshot + transcript-so-far, then live deltas, with stable word/segment ids and Partial→Final states
- [ ] The caption subscription is opt-in and lossy: a deliberately slow client gets conflated captions; capture and journaling are provably unaffected
- [ ] The Whisper model loads via ticket 05's registry; a missing model yields a legible not-ready error, never a crash
- [ ] Finalized Meeting's Mirror now contains the real Transcript section; FTS search finds spoken words
- [ ] WER on the bilingual fixture is computed and reported by the test run (number tracked, not gated)
