# 01: The sidecar fits the machine it runs on

**What to build:** The Summary sidecar loads a model in a way that fits the machine,
and says which choice it made. M4's `04-local-sidecar` claims the sidecar is "spawned at
reduced priority and with a layers-that-fit calculation"; neither exists — the load uses
default parameters. At 491 MB nobody noticed. This lands first because a 4B with every
layer offloaded by default is what makes the omission bite.

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [ ] The sidecar computes how many layers to offload from the model's own metadata and the machine's memory, rather than accepting the default that offloads everything
- [ ] Generation runs at reduced priority, so a Summary cannot contend with a live recording for CPU
- [ ] The chosen layer count and priority appear in the sidecar's diagnostics, so "it fits" is observable rather than assumed
- [ ] A machine that cannot fit the model at all fails with a reason naming that, not a generic unavailability
- [ ] M4's criterion is un-ticked until this is true, and its text says what was actually missing
- [ ] The local gate is green
