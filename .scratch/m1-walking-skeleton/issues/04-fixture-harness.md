# 04: Fixture harness

**What to build:** The shared test harness every later milestone rides: real bilingual meeting audio as compile-time fixtures, feature-based audio-similarity assertions, WER/CER metrics, and hallucination canaries — proven by an end-to-end test that plays a fixture through the AudioSource seam into a finished Meeting with a playable AAC file.

**Blocked by:** 03 (the AudioSource seam must exist).

**Status:** done

- [x] Fixture crate with at least two real meeting clips (one zh/en code-switching) at multiple sample rates, exposed as compile-time constants
- [x] Audio-similarity assertion library (RMS, peak, zero-crossing, spectral centroid, band energies — tolerances, not bit-exactness)
- [x] WER/CER computation available to tests (ASR quality numbers become a tracked deliverable)
- [x] Silence-heavy and noise-heavy canary fixtures for the hallucination suite (consumed by ticket 07)
- [x] RMS-preservation helper for resampling checks (consumed by ticket 08)
- [x] End-to-end: fixture → AudioSource → checkpointed AAC → finalized Meeting, green on both platforms in CI
