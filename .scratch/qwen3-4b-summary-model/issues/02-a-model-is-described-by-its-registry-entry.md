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

**Status:** ready-for-agent

- [ ] A registry entry carries how to frame a prompt for that model, how to sample, whether to suppress reasoning, and its context budget
- [ ] Entries carry licence and source; all four existing models are filled in, not only the one about to change
- [ ] The sidecar drives a model from those properties rather than from hardcoded constants
- [ ] Raw framing stays expressible for a model with no embedded chat template, since applying one has no fallback
- [ ] Summaries from the registered 0.5B are unchanged — asserted, not assumed
- [ ] The entry data is asserted without loading a model
- [ ] The local gate is green
