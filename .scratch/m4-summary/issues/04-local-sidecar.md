# 04: The local sidecar — llama.cpp in its own process

**What to build:** ADR-0031's bundled local Backend, so "local by default" holds on a fresh machine with nothing else installed.

**Blocked by:** 01.

Status: not started

- [ ] A separate binary embedding llama.cpp, spawned and supervised by the Core, speaking JSONL over stdio (ADR-0031, catalog M4)
- [ ] **A process boundary, not a library.** The ADR's evidence is that the one competitor who embedded llama.cpp in-process abandoned it — and the Core is the thing that must never die, because it is the thing that is recording
- [ ] Lifecycle from the catalog rather than invented: lazy model load on first `generate` (skipped when path and context match), keep-alive ping that is skipped while busy, idle self-exit, **stdin-EOF as orphan protection**, per-request timeout, and graceful drain then SIGKILL
- [ ] **Kill-as-cancel**, because llama.cpp cannot be interrupted mid-generation. Cancellation that politely asks and then waits is not cancellation
- [ ] **Incremental UTF-8 decode**, so a CJK character split across two tokens is not corrupted. This product has already paid for Chinese handling once (DECISIONS Q11–Q13) and the transcript it is summarizing is routinely Chinese
- [ ] Spawned at reduced priority and with a layers-that-fit calculation (catalog M4): a Summary must never contend with a live recording. M1 already paid for the version of this where transcription starved capture (DECISIONS Q7)
- [ ] The model joins the checksummed download set, like every other model (ADR-0034 unchanged — no new host, no new path)
- [ ] An installed Ollama or LM Studio is detected and preferred when present (ADR-0031): same OpenAI-compatible abstraction, different base URL
- [ ] Degradation is honest: no model, no sidecar, or a crashed sidecar leaves the Meeting without a Summary and says so. It never costs the recording, the Transcript, or the attribution
- [ ] **It runs on Windows, demonstrated rather than assumed.** A second binary is a second thing to build, locate and launch on two platforms, and M2 ended by finding a whole platform that had never worked while CI was green
