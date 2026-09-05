# The default local Summary backend is a bundled llama.cpp sidecar

> **Amended 2026-09-05: the auto-detection clause is dropped.** An installed
> Ollama or LM Studio is *offered*, not detected and preferred. Everything
> else below stands.
>
> The clause promised that a local server "is auto-detected and preferred when
> present", and preferring is the part that does not survive contact with
> ADR-0013. A server appearing on `localhost:11434` would silently change
> which model summarises the Operator's meetings — same Backend name, same
> "Local" in the picker, different engine than yesterday, and nothing anywhere
> saying so. ADR-0013's subject is that the Backend is the Operator's to
> choose; its 2026-09-01 amendment narrowed that to *one* disclosed
> preselection on a fresh install, not to switching engines underneath someone
> at runtime.
>
> It also buys little. The sentence's own justification — "same
> OpenAI-compatible abstraction, different base URL" — is why detection is
> unnecessary: Ollama and LM Studio are already two entries in
> `summary::cloud::PRESETS` beside OpenAI and Anthropic, reached by the same
> client, and `is_loopback` already classifies them as local so they raise no
> exfiltration warning and add no Sanctioned Traffic. Choosing one is a click.
> Against that, detection costs a background probe of localhost ports, a
> preference rule, and a silent change to where meeting content goes.
>
> If discoverability is the goal, the honest form is to *show* that a local
> server was found and let the Operator pick it. That is a UI affordance
> nobody has asked for yet, and it is not this decision.

"A small local model is the default Backend" must hold on a fresh machine with nothing else installed, so the local Summary engine ships in the box: a **Core-supervised sidecar binary** embedding llama.cpp, speaking JSON over stdio, spawned on demand with idle-timeout shutdown and crash isolation from the Core. Its small instruct model downloads during onboarding when the Operator picks Local. An installed Ollama or LM Studio is offered in the Backend picker — same OpenAI-compatible abstraction, different base URL — and Apple Foundation Models is a post-v1 opportunistic tier.

Evidence set the shape: Meetily ships exactly this sidecar (spawn-on-demand, JSON-stdio, keep-alive/idle-timeout) successfully at consumer scale, while anarlog stubbed out its **in-process** llama.cpp server and retreated to external runtimes — a warning against in-process embedding, answered by the process boundary, not against bundling.

## Considered options

Requiring Ollama/LM Studio (a fresh install has no working local Summary until the Operator installs a second product — the default-local promise quietly breaks) and in-process llama.cpp (anarlog's abandoned path) were rejected.

## Consequences

- Onboarding's Local path gains a ~2–4GB model download, explained at the moment it's demanded (ADR-0011).
- Distribution bundles the sidecar; the updater covers it like the Core.
- GPU contention stays bounded as before: Summary runs post-meeting (ADR-0014's profiling note stands).
