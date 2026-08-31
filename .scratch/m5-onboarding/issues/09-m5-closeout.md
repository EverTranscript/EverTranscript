# 09: M5 close-out — v1 readiness

**What to build:** The evidence that someone who is not the author can install this and understand what it does.

**Blocked by:** 01–08.

Status: not started

- [ ] **A clean-machine install, by someone who did not build it.** Every prior close-out found defects by running the real thing on real input, and every prior set of unit tests had passed. The M5 form is an Operator who is not the author, installing from a package, on a machine that has never built this — and it is the one thing this repository cannot self-serve
- [ ] The Briefing read by someone other than its author, who can then say what the product does. If they cannot, it failed at the only job it has
- [ ] Onboarding completed end to end on a machine with no models, no permissions, and no keys — the state every real first run is in and no development machine ever is
- [ ] **The zero-network guarantee in its final form**: updates off, models downloaded, a full record-transcribe-diarize-summarize cycle, and no traffic at all
- [ ] The permission set on a signed bundle matches what the audit asserts, checked against the built artifact rather than the source
- [ ] Both platforms installed and run from their real artifacts
- [ ] The accumulated open items from M1–M4 reviewed and either closed, carried into a v1.1 list, or explicitly abandoned — a milestone that ships with four milestones of quiet debt is a milestone that shipped a lie about its state
- [ ] **A statement of what v1 is not.** Every milestone has left named gaps; a release that does not enumerate them hands the next person a discovery instead of a list
