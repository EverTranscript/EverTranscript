# First-run is linear setup; configuration never prompts at runtime

First-run is linear: the Briefing (legal education + acknowledgment, voice-profiling disclosure folded in), then the Summary Backend choice, then satisfying exactly that choice's requirements — the Whisper model download for everyone (the Anchor is local), plus a cloud key if cloud Summary was chosen. The wall is as tall as the chosen configuration, every demand is explained at the moment it's made, and the Operator exits setup fully armed.

The invariant this buys: configuration happens only in setup and Settings. Features never pop configuration prompts at runtime; an unconfigured feature shows a legible "not configured" state instead.

## Considered options

Record-first lazy configuration (instant notetaker value via zero-download ASR, per-feature just-in-time setup) was recommended and rejected: predictability beat time-to-first-value. A full guided tour with sample audio was rejected as maximal drop-off teaching on data the Operator doesn't care about.
