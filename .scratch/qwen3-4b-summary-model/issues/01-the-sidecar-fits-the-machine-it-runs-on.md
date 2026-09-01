# 01: The sidecar fits the machine it runs on

**What to build:** The Summary sidecar loads a model in a way that fits the machine,
and says which choice it made. M4's `04-local-sidecar` claims the sidecar is "spawned at
reduced priority and with a layers-that-fit calculation"; neither exists — the load uses
default parameters. At 491 MB nobody noticed. This lands first because a 4B with every
layer offloaded by default is what makes the omission bite.

**Blocked by:** None (can start immediately).

**Status:** done

- [x] Computed from the model's block count and the machine's memory. **The layer count must come from GGUF metadata, not `n_layer()`** — a vocab-only load reports zero layers, which the first version believed, silently turning a Metal load into a CPU one while every test stayed green. Caught by running it: 19.1s versus 2.8s for the same generation
- [x] The process lowers its own scheduling priority at startup, before anything expensive — nice 10 on unix, below-normal on Windows. Best-effort: a platform that refuses runs at normal priority rather than failing
- [x] Both reported on stderr, which the Core inherits. This is what made the metadata bug visible at all, and it is the answer to how the original criterion stayed false for a milestone
- [x] A load that fails with nothing offloaded reports the weights and the memory rather than a bare error
- [x] M4's criterion now records that it was ticked falsely for a milestone, what was missing, and that it is true
- [x] Seven pure tests for the decision plus a real load; the local gate is green
