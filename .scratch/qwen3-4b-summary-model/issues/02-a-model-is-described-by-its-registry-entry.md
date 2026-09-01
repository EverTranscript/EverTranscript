# 02: A model is described by its registry entry

**What to build:** How a model wants to be driven becomes data next to its checksum
instead of constants in the sidecar: prompt framing, sampling settings, whether to
suppress reasoning, and its context budget. Every entry also records its licence and
source, which four models currently lack in a public Apache-2.0 project.

**This ticket changes no behaviour.** The registered 0.5B is described exactly as the
sidecar treats it today — raw framing, greedy sampling, the current context size — so
summaries come out identical. That is what makes it safe to swap the model in 03 without
also debugging a new seam.

**Blocked by:** 01 (both touch the sidecar's load path; sequencing avoids a collision).

**Status:** done

- [x] All four, plus the single-pass threshold, carried on the entry and sent to the sidecar with the load — the sidecar is a separate process by design, so the properties travel with the path rather than being read from a registry it cannot see
- [x] All four filled in, asserted by a test that every entry has a licence and a followable source. Recorded per entry rather than in `PORTS.md`, whose discipline is file-level — attribution headers and upstream revisions, neither of which a downloaded model has
- [x] Framing, sampling, reasoning suppression and context all come from the message; `CONTEXT_TOKENS` is gone. Reasoning suppression joins the system turn from the entry, never the Operator's editable prompt
- [x] `Framing::Plain` is a first-class choice, and the model currently registered uses it
- [x] Run against the real model: same `None noted.`, same invented timestamps, same 3/3 verbatim echo Q45 recorded. A test also asserts the entry's description equals the sidecar's previous behaviour, so the no-op claim is checkable rather than trusted — it is written to fail when the model changes, which is the moment someone must choose deliberately
- [x] Provenance, and that only the prompted model describes driving — a sampling temperature on the ONNX pair would be meaningless
- [x] The local gate is green
