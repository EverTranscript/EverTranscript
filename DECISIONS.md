<!-- AI-maintained, append-only -->

## Q1 — m1/08-aec-dsp-quality — deviation

**Question:** ADR-0029 specifies DTLN-shaped echo cancellation. Should the implementation ship the DTLN models and an inference runtime, or a classic adaptive filter?

**Options considered:** DTLN ONNX models plus an inference runtime (as ratified) / normalized least-mean-squares adaptive filter with a residual suppressor / leave echo cancellation unimplemented and keep recording the ticket gap.

**Chosen:** NLMS adaptive filter (128 ms) followed by a residual echo suppressor, no models and no inference runtime.

**Decided-by:** agent

**Justification:** The usual reason to prefer a learned canceller is alignment — the reference and microphone normally arrive on different clocks with an unknown, drifting delay that has to be estimated before any filter converges. ADR-0029's other half removes that problem here: both legs are stamped on one capture clock and reach the pipeline aligned to the sample, so what is left is the case a linear adaptive filter handles well. Against that, DTLN costs an ONNX runtime, a model to download and version, and inference budget competing with transcription on the same machine. Measured on synthetic echo fixtures: echo-only input is driven to silence, clean audio is preserved at 100.0% of its level, and 115% of near-end power survives double talk. End to end with the real engine, an uncancelled speakerphone reproduces the far end on the microphone channel faithfully (WER 0.08 against the far-end transcript) while the cancelled one does not (WER 0.86). Revisit if real speakerphone recordings show the linear filter failing on nonlinear speaker distortion, which is the case DTLN is genuinely better at and which synthetic fixtures cannot exhibit.

**Outcome:** applied

**Ref:** (pending)

## Q2 — m1/08-aec-dsp-quality — tradeoff

**Question:** A linear filter alone still leaves an echo that a transcription model decodes perfectly well. Accept partial reduction, or suppress the residual and risk attenuating genuine near-end sound?

**Options considered:** linear cancellation only, accepting that a quiet echo still reaches the record / add a residual suppressor gated on echo dominance / suppress whenever the far end is playing (half-duplex).

**Chosen:** Suppress the residual, but only while the filter is demonstrably explaining most of the microphone's energy.

**Decided-by:** agent

**Justification:** The record does not care how many decibels down an echo is — an intelligible echo is transcribed and attributed to the Operator either way, which is the harm ADR-0029 names. Gating on echo dominance rather than on far-end activity is what keeps it from being half-duplex: the Operator's voice is not predictable from the reference, so during double talk the residual stays large and no suppression happens. The cost is that a quiet non-echo sound picked up while the far end is playing may be attenuated with it — the standard tradeoff for this stage, and preferable to crediting people with words they did not say.

**Outcome:** applied

**Ref:** (pending)

## Q3 — m1/03-capture-vertical — gate-resolution

**Question:** macOS grants a process tap whether or not the Operator has allowed audio recording, then delivers digital silence forever when they have not. With no API distinguishing the two, how should a refused permission be reported?

**Options considered:** trust the tap-creation return code / query TCC directly / detect the silence at runtime and report it.

**Chosen:** Detect it during capture: frames arriving steadily while no sample has ever been non-zero.

**Decided-by:** agent

**Justification:** There is no public API for the audio-capture permission's state, and the tap's success proves nothing. What makes detection sound rather than a guess is an asymmetry confirmed on this machine: a global tap's callback fires only while something is playing, so a machine with nothing to record delivers no frames at all. Frames arriving while every sample is bit-exact zero therefore means audio is being played and we are being handed zeros — not that the room is quiet. One real non-zero sample ends the check for the session. Verified against the actual failure: this machine has no grant, and the detector fires with an actionable message after 15 seconds of played-but-silent audio.

**Outcome:** applied

**Ref:** (pending)

## Q4 — m1/03-capture-vertical — deviation

**Question:** A recording that lost a capture leg recorded that fact only to a log line. Should the loss reach the record itself, which means a schema migration and a protocol field neither ticket asked for?

**Options considered:** leave it in the log and let the Operator infer / add a note to the Mirror only / carry it on the Meeting through store, protocol, Mirror, CLI and client.

**Chosen:** Carry it on the Meeting: a new `audio_notes` column (migration 4), an additive `audioNotes` protocol field, and rendering in the Mirror, `show`, and the Electron client.

**Decided-by:** agent

**Justification:** Not asked for by tickets 03 or 09, but it is what makes the rest of their work legible. A Meeting recorded with one leg missing produced a one-sided transcript and nothing anywhere to explain it — indistinguishable, to the person reading their notes a week later, from a meeting where nobody else spoke. On any machine without the system-audio permission that is every meeting, so the gap is not an edge case. The protocol change is additive, which ADR-0028 permits. Verified end to end against a real daemon on this machine, which has neither leg available: both reasons reach `show` and the Mirror.

**Outcome:** applied

**Ref:** (pending)
