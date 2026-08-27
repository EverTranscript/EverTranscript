# The ASR anchor is whisper.cpp behind a thin, owned Swift wrapper

> **Amended by ADR-0025:** the wrapper is now Rust (whisper-rs-style bindings over the same C API), not Swift, and Metal is the macOS half of per-platform GPU backends. Everything else stands — whisper.cpp itself, the owned thin-wrapper posture, and the in-house meeting-grade streaming layer.

Live transcription runs on whisper.cpp via a hand-rolled Swift wrapper over its C API, in the style of OpenSuperWhisper's `MyWhisperContext` (Starmel/OpenSuperWhisper, `OpenSuperWhisper/Whis/Whis.swift`). Full control, the GGML model zoo (strong multilingual including Chinese), no Apple-framework OS floor (macOS 13+ instead of 26+), and battle-tested prior art across the VoiceInk / Vibe / Meetily lineage.

## Considered options

Apple SpeechAnalyzer (zero-download, OS-maintained, but macOS 26+ floor and Apple-owned quality) and WhisperKit (CoreML/ANE, but a framework dependency where a ~40-line C wrapper suffices) were both declined.

## Consequences

- Every first-run path gains a Whisper model download — even All Cloud, since the Anchor is local for everyone. This lands as an explained step in the Profile-led setup (ADR-0011).
- whisper.cpp is not natively streaming: meeting-grade live captions (VAD + sliding-window chunking over hour-long audio) are ours to build. The cited prior art is push-to-talk dictation — it never had to solve this.
- whisper.cpp runs on Metal, and so does the local Summary model — but Summary runs post-meeting, so the two contend only if a Summary is still generating when the next recording starts. Profile in M3 anyway.
