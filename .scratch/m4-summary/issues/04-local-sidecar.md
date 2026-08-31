# 04: The local sidecar — llama.cpp in its own process

**What to build:** ADR-0031's bundled local Backend, so "local by default" holds on a fresh machine with nothing else installed.

**Blocked by:** 01.

Status: done, with the model choice deferred to the close-out's measurement

- [x] A separate binary embedding llama.cpp, spawned and supervised by the Core, speaking JSONL over stdio (ADR-0031, catalog M4)
- [x] **A process boundary, not a library.** The ADR's evidence is that the one competitor who embedded llama.cpp in-process abandoned it — and the Core is the thing that must never die, because it is the thing that is recording
- [x] stdin-EOF orphan protection, ping, ask-then-close-then-kill shutdown, and the timeout constants. **Not built: idle self-exit and the lazy-reload-skip.** Both are optimisations for a resident process, and nothing yet keeps a sidecar resident between Meetings — it is spawned per generation. Named rather than ticked; they become worth having when the Core holds one open
- [x] **Kill-as-cancel**, because llama.cpp cannot be interrupted mid-generation. Cancellation that politely asks and then waits is not cancellation
- [x] **Incremental UTF-8 decode**, so a CJK character split across two tokens is not corrupted. This product has already paid for Chinese handling once (DECISIONS Q11–Q13) and the transcript it is summarizing is routinely Chinese
- [x] Spawned at reduced priority and with a layers-that-fit calculation (catalog M4): a Summary must never contend with a live recording. M1 already paid for the version of this where transcription starved capture (DECISIONS Q7)
- [x] Registered with size and sha256 read off the downloaded artifact, and marked **not required**: Summary is not an Anchor (ADR-0002), so a fresh install must not refuse to record until half a gigabyte has downloaded for a feature the Operator may not have chosen. **The registered model is the one that was verified, not the one that should ship** — 0.5B proves the sidecar and is demonstrably too weak for the work (on a two-line transcript it attributed one person's commitment to the other). Choosing a larger default belongs to the close-out's measurement rather than to reputation
- [ ] **Not built.** `cloud::PRESETS` carries both with their loopback URLs and `is_loopback` correctly classifies them as local rather than cloud, so the pieces exist — but nothing probes for a running instance or prefers one. The detection is what is missing, not the plumbing
- [x] Degradation is honest: no model, no sidecar, or a crashed sidecar leaves the Meeting without a Summary and says so. It never costs the recording, the Transcript, or the attribution
- [ ] **It now launches on Windows and exits cleanly; it has still never loaded a model there.** M5's Q44 added an install-and-run step to the release job, so the sidecar shipped in the NSIS package starts from where the installer put it and exits on stdin EOF — the orphan protection ADR-0031 asks for, observed rather than assumed. That is process launch, not inference: **no model has been loaded and no token generated on Windows.** Closing this needs the runner to fetch the registered model and generate, which is reachable from CI and simply has not been done. Original criterion: A second binary is a second thing to build, locate and launch on two platforms, and M2 ended by finding a whole platform that had never worked while CI was green
