# 07: The Knob — explicit choice, one-way fallback, Strict Mode

**What to build:** The policy that decides which Backend runs, and the guarantees around it. **This is the ticket where a bug leaks meeting content**, and it should be read that way.

**Blocked by:** 01, 05, 06.

Status: done

- [x] **No preselection** (ADR-0013): the Backend picker offers Local (badged Recommended) and Cloud, and neither is chosen until the Operator chooses. Every configuration the product runs traces to an explicit act
- [x] **Choosing Cloud triggers a hard one-time warning** (story 36, ADR-0013) naming what leaves the machine
- [x] **Fallback runs cloud→local only, and the asymmetry is structural rather than conditional** (story 38): there is no code path from local to cloud, so no bug, retry or timeout can produce one. A boolean that happens to be false is not the same guarantee, and the tests should be able to tell the difference
- [x] **Strict Mode** (story 39) disables even the permitted direction: on failure the Operator is told, and nothing switches
- [x] The **active Backend is always visible** (story 38) — which one is running now, not merely which was configured
- [x] A fallback is reported, not silent. An Operator who chose Cloud and got local Summary quality must know why
- [x] Fallback is driven by **real failure shapes** in tests: refused connection, 401, timeout mid-stream, malformed response
- [x] Switching the Knob mid-generation does not corrupt the Meeting in progress. *Left open until 2026-09-15*, because nothing could switch the Knob before ticket 08. A run reads the Knob once and keeps the Backends it built, so a switch applies to the next run and never to the one under way (`DECISIONS.md` Q109). Checked by `switching_the_knob_mid_generation_leaves_the_run_alone`, which switches Cloud to Local while the first chunk is being generated: the cloud Backend serves every chunk and the reduce, the record is labelled with it, the local Backend is never asked, and the new setting holds for the next run
