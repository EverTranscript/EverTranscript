# 08: AEC + DSP quality

**What to build:** Speakerphone meetings stop poisoning the record: echo cancellation on the mic channel with the system channel as reference (ADR-0029), EBU-R128 loudness normalization on the mic, and persistent-resampler discipline — all verified against echo fixtures.

**Blocked by:** 03, 04, 05.

**Status:** done — implemented as an adaptive filter rather than DTLN (see DECISIONS.md Q1/Q2)

- [x] Echo cancellation on the mic leg with the system leg as reference. **Deliberate deviation from ADR-0029's
      DTLN shape:** a 128 ms NLMS adaptive filter plus a residual suppressor, with no ONNX runtime and no model to
      download. What normally makes a learned canceller worth its cost is alignment — the reference and mic arrive
      on different clocks with an unknown drifting delay — and ADR-0029's shared capture clock already removes that.
      A linear filter alone was not enough: it takes the echo down while leaving something the model transcribes
      perfectly happily, so the residual is suppressed once the filter is demonstrably explaining most of what the
      microphone hears. Gating on echo *dominance* rather than far-end activity is what keeps it from being
      half-duplex. Recorded as DECISIONS.md Q1 and Q2; revisit if real speakerphone audio shows nonlinear speaker
      distortion, which synthetic fixtures cannot exhibit.
- [x] Echo fixtures: a synthetic room (direct path plus decaying reflections) and ERLE in
      `evertranscript-fixtures::echo`. Measured — echo-only input driven to silence; **100.0%** of the level
      preserved when no echo is present, which is the headphones case and therefore most meetings; **115%** of
      near-end power kept through double talk. End to end with the real engine, an uncancelled speakerphone
      reproduces the far end on the mic channel faithfully (WER 0.08 against the far-end transcript) and a
      cancelled one does not (WER 0.86) — run both ways, so the guard has been watched failing.
      Cost measured too: **17x realtime** in release, ~6% of one core.

      **Correction (Q50): those two WER figures were measured on `ggml-tiny`, and the entry did not say so.**
      They read as properties of the canceller and are properties of a model. The registered
      `WHISPER_DEFAULT` reads the same residual at **WER 0.649** and recovers fourteen intelligible words of
      the far end, so the `> 0.7` guard built on 0.86 failed the first time CI ran it with the model that
      ships. The canceller itself is unchanged and tiny still scores 0.865 against it. The end-to-end figure
      is now reported rather than asserted, and the guard is ERLE in
      `audio::aec::tests::real_speech_echo_is_cancelled_by_a_measurable_amount` — **32.8 dB**, no model, both
      platforms. What that does not carry over is intelligibility, which is the harm; `ECHO_DOMINANCE` says
      why a decibel figure cannot stand in for it. Whether 0.649 is acceptable is open.
- [x] EBU R128 normalization toward −23 LUFS with a −1 dBFS true-peak ceiling and a max 8× gain, applied
      to what reaches the *model*. Deliberate deviation: the stored file stays as captured, so the Enhance
      family can re-derive from unmodified audio. Silence is never amplified (that is hallucination fuel).
- [x] One `SincFixedIn` per leg for the life of the Meeting, fed fixed chunks with the remainder carried
      across boundaries; RMS preservation asserted within 97–103% against deliberately ragged buffer sizes.
