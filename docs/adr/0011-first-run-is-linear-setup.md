# First-run is linear setup; configuration never prompts at runtime

First-run is linear: the Briefing (legal education + acknowledgment, voice-profiling disclosure folded in), then the Summary Backend choice, then satisfying exactly that choice's requirements — the Whisper model download for everyone (the Anchor is local), plus a cloud key if cloud Summary was chosen. The wall is as tall as the chosen configuration, every demand is explained at the moment it's made, and the Operator exits setup fully armed.

The invariant this buys: configuration happens only in setup and Settings. Features never pop configuration prompts at runtime; an unconfigured feature shows a legible "not configured" state instead.

## Considered options

Record-first lazy configuration (instant notetaker value via zero-download ASR, per-feature just-in-time setup) was recommended and rejected: predictability beat time-to-first-value. A full guided tour with sample audio was rejected as maximal drop-off teaching on data the Operator doesn't care about.

> **Amended 2026-09-18: setup gains a skippable step that records the
> Operator's voice.**
> *Considered options* above rejects "a full guided tour with sample audio" as
> maximal drop-off teaching on data the Operator does not care about. The
> enrolment step is narrower than what was rejected — one screen, one button,
> a visible Skip, and it teaches nothing — but it is a step added to a linear
> setup, and this ADR is where that is accountable.
>
> It sits after the model download rather than beside the Briefing, because it
> runs both diarization models over what it records: a step that can only fail
> until a download finishes is a step that teaches people to skip.
>
> Two things keep the invariant intact. It is genuinely skippable — an
> installation that skips it identifies the Operator exactly as before — and
> the Registry carries the same control afterwards, so this is configuration
> reachable from setup *and* Settings, which is what this ADR asks of every
> feature.
>
> The Briefing grows a paragraph with it. ADR-0011 folds voice-profiling
> disclosure into the Briefing, and that copy describes a *mathematical
> fingerprint* of a voice. An enrolment keeps the audio itself, indefinitely,
> so that a voice-model change can re-embed it instead of costing the Operator
> their identity. Agreeing to a fingerprint is not agreeing to a recording,
> and the Briefing now says both.
