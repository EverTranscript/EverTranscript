# 08: AEC + DSP quality

**What to build:** Speakerphone meetings stop poisoning the record: DTLN-shape echo cancellation on the mic channel with the system channel as reference (ADR-0029), EBU-R128 loudness normalization on the mic, and persistent-resampler discipline — all verified against echo fixtures.

**Blocked by:** 03, 04, 05.

**Status:** ready-for-agent

- [ ] AEC (ONNX via ort, models from ticket 05) runs on the mic channel with the system channel as echo reference; alignment change resets the canceller
- [ ] Echo fixture (far-end audio bleeding into the mic) shows the double-transcription eliminated relative to the no-AEC baseline
- [ ] Mic channel normalized to −23 LUFS (±1) with true-peak limiting; channels persist post-processing (ADR-0029: raw pre-AEC audio is never kept)
- [ ] One persistent resampler per stream fed fixed-size chunks; RMS preservation asserted within 97–103% (the per-chunk-construction bug class is regression-tested)
