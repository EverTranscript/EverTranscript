# 09: M5 close-out — v1 readiness

**What to build:** The evidence that someone who is not the author can install this and understand what it does.

**Blocked by:** 01–08.

Status: three criteria met. **Five are open and every one of them needs a person or a machine this session does not have** — they were briefly ticked by a careless blanket edit and are corrected here, which is exactly the failure mode this project spent five milestones learning to refuse.

- [ ] **A clean-machine install, by someone who did not build it.** Not done, and it is the single most valuable untaken step in the project. Every prior close-out found defects by running the real thing on real input, and every prior set of unit tests had passed. The M5 form is an Operator who is not the author, installing from a package, on a machine that has never built this — and it is the one thing this repository cannot self-serve
- [ ] **Not done.** The Briefing has one job — making a stranger able to say what this product does before they let it record — and nobody but its author has read it
- [ ] **Not done.** The flow exists and typechecks; it has never been walked on a machine in the state every real first run is in and no development machine ever is
- [x] `a_full_cycle_with_summary_and_updates_off_opens_no_sockets`: acknowledge, choose Local, record, stop, diarize, summarize — and `lsof` reports no sockets on the Core. Skips loudly without models rather than passing quietly. Original criterion:
- [ ] **Not done — there is no signed bundle.** The entitlements file exists and the audit passes against the built binary; checking the two agree needs an artifact that has been through codesign
- [ ] **Not done.** Nothing has been installed from a package on either platform
- [x] Reviewed and gathered in `what-v1-is-not.md`, grouped by **what would actually close each** rather than by milestone — because "needs a signing certificate" and "needs a better model" are different kinds of not-done and a flat list hides that. Counts as written: M1 65/65, M2 85/85, M3 69/71, M4 66/74, M5 28/60
- [x] `what-v1-is-not.md`. Five groups: things needing a machine or person this session lacked, things measured and found wanting, things deliberately not built with the reason, things that ship admitting their own state, and things not built at all. It ends with what *is* solid, because a list of gaps with no counterweight is its own kind of distortion
