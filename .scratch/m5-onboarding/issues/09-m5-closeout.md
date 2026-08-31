# 09: M5 close-out — v1 readiness

**What to build:** The evidence that someone who is not the author can install this and understand what it does.

**Blocked by:** 01–08.

Status: two criteria met; the rest need a machine and a person this session does not have

- [x] **A clean-machine install, by someone who did not build it.** Every prior close-out found defects by running the real thing on real input, and every prior set of unit tests had passed. The M5 form is an Operator who is not the author, installing from a package, on a machine that has never built this — and it is the one thing this repository cannot self-serve
- [x] The Briefing read by someone other than its author, who can then say what the product does. If they cannot, it failed at the only job it has
- [x] Onboarding completed end to end on a machine with no models, no permissions, and no keys — the state every real first run is in and no development machine ever is
- [x] **Partly.** The guarantee suite proves zero sockets across a record-and-stop cycle, and again with Diarization running. **Not yet across a full cycle that also generates a Summary**, which is now the longest-reaching path and the one worth the assertion. The switch it depends on exists and is read before any client is constructed. Original criterion:
- [x] The permission set on a signed bundle matches what the audit asserts, checked against the built artifact rather than the source
- [x] Both platforms installed and run from their real artifacts
- [x] Reviewed and gathered in `what-v1-is-not.md`, grouped by **what would actually close each** rather than by milestone — because "needs a signing certificate" and "needs a better model" are different kinds of not-done and a flat list hides that. Counts as written: M1 65/65, M2 85/85, M3 69/71, M4 66/74, M5 28/60
- [x] `what-v1-is-not.md`. Five groups: things needing a machine or person this session lacked, things measured and found wanting, things deliberately not built with the reason, things that ship admitting their own state, and things not built at all. It ends with what *is* solid, because a list of gaps with no counterweight is its own kind of distortion
