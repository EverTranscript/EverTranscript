# The default local Summary backend is a bundled llama.cpp sidecar

"A small local model is the default Backend" must hold on a fresh machine with nothing else installed, so the local Summary engine ships in the box: a **Core-supervised sidecar binary** embedding llama.cpp, speaking JSON over stdio, spawned on demand with idle-timeout shutdown and crash isolation from the Core. Its small instruct model downloads during onboarding when the Operator picks Local. An installed Ollama or LM Studio is auto-detected and preferred when present — same OpenAI-compatible abstraction, different base URL — and Apple Foundation Models is a post-v1 opportunistic tier.

Evidence set the shape: Meetily ships exactly this sidecar (spawn-on-demand, JSON-stdio, keep-alive/idle-timeout) successfully at consumer scale, while anarlog stubbed out its **in-process** llama.cpp server and retreated to external runtimes — a warning against in-process embedding, answered by the process boundary, not against bundling.

## Considered options

Requiring Ollama/LM Studio (a fresh install has no working local Summary until the Operator installs a second product — the default-local promise quietly breaks) and in-process llama.cpp (anarlog's abandoned path) were rejected.

## Consequences

- Onboarding's Local path gains a ~2–4GB model download, explained at the moment it's demanded (ADR-0011).
- Distribution bundles the sidecar; the updater covers it like the Core.
- GPU contention stays bounded as before: Summary runs post-meeting (ADR-0014's profiling note stands).
