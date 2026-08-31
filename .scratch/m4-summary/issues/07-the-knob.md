# 07: The Knob — explicit choice, one-way fallback, Strict Mode

**What to build:** The policy that decides which Backend runs, and the guarantees around it. **This is the ticket where a bug leaks meeting content**, and it should be read that way.

**Blocked by:** 01, 05, 06.

Status: done, except the mid-generation switch named below

- [x] **No preselection** (ADR-0013): the Backend picker offers Local (badged Recommended) and Cloud, and neither is chosen until the Operator chooses. Every configuration the product runs traces to an explicit act
- [x] **Choosing Cloud triggers a hard one-time warning** (story 36, ADR-0013) naming what leaves the machine
- [x] **Fallback runs cloud→local only, and the asymmetry is structural rather than conditional** (story 38): there is no code path from local to cloud, so no bug, retry or timeout can produce one. A boolean that happens to be false is not the same guarantee, and the tests should be able to tell the difference
- [x] **Strict Mode** (story 39) disables even the permitted direction: on failure the Operator is told, and nothing switches
- [x] The **active Backend is always visible** (story 38) — which one is running now, not merely which was configured
- [x] A fallback is reported, not silent. An Operator who chose Cloud and got local Summary quality must know why
- [x] Fallback is driven by **real failure shapes** in tests: refused connection, 401, timeout mid-stream, malformed response
- [ ] **Not built.** The Knob is a value read at the start of a run, so a switch during one cannot affect it — but nothing yet *persists* the Knob or lets it be switched at all, which is ticket 08's surface. Left open rather than ticked on an argument: the criterion asks for a behaviour under a concurrent act that no code path can currently perform
