# 07: The Knob — explicit choice, one-way fallback, Strict Mode

**What to build:** The policy that decides which Backend runs, and the guarantees around it. **This is the ticket where a bug leaks meeting content**, and it should be read that way.

**Blocked by:** 01, 05, 06.

Status: not started

- [ ] **No preselection** (ADR-0013): the Backend picker offers Local (badged Recommended) and Cloud, and neither is chosen until the Operator chooses. Every configuration the product runs traces to an explicit act
- [ ] **Choosing Cloud triggers a hard one-time warning** (story 36, ADR-0013) naming what leaves the machine
- [ ] **Fallback runs cloud→local only, and the asymmetry is structural rather than conditional** (story 38): there is no code path from local to cloud, so no bug, retry or timeout can produce one. A boolean that happens to be false is not the same guarantee, and the tests should be able to tell the difference
- [ ] **Strict Mode** (story 39) disables even the permitted direction: on failure the Operator is told, and nothing switches
- [ ] The **active Backend is always visible** (story 38) — which one is running now, not merely which was configured
- [ ] A fallback is reported, not silent. An Operator who chose Cloud and got local Summary quality must know why
- [ ] Fallback is driven by **real failure shapes** in tests: refused connection, 401, timeout mid-stream, malformed response
- [ ] Switching the Knob mid-generation does not corrupt the Meeting in progress
