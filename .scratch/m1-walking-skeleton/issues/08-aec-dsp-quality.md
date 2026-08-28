# 08: AEC + DSP quality

**What to build:** Speakerphone meetings stop poisoning the record: DTLN-shape echo cancellation on the mic channel with the system channel as reference (ADR-0029), EBU-R128 loudness normalization on the mic, and persistent-resampler discipline — all verified against echo fixtures.

**Blocked by:** 03, 04, 05.

**Status:** partly done — echo cancellation not implemented

- [ ] **Not done.** Needs the DTLN ONNX models and an inference runtime, neither of which this build ships.
      This is a real gap against ADR-0029, not a deferral: on speakers the far end re-enters the microphone,
      which breaks the channel attribution and double-transcribes remote speech.
- [ ] **Not done** — blocked on the same missing models. The echo fixture is worth building alongside them.
- [x] EBU R128 normalization toward −23 LUFS with a −1 dBFS true-peak ceiling and a max 8× gain, applied
      to what reaches the *model*. Deliberate deviation: the stored file stays as captured, so the Enhance
      family can re-derive from unmodified audio. Silence is never amplified (that is hallucination fuel).
- [x] One `SincFixedIn` per leg for the life of the Meeting, fed fixed chunks with the remainder carried
      across boundaries; RMS preservation asserted within 97–103% against deliberately ragged buffer sizes.
