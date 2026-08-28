# 07: Caption quality + resilience kit

**What to build:** The transcript becomes trustworthy under adverse conditions: the layered hallucination defenses, the prefix-commit caption stabilizer with its gap-skip clock, tail-integrity accounting so a stop never silently loses the last sentence, and the ADR-0029 degradation contract — a dead ASR leg never stops the recording.

**Blocked by:** 04, 06.

**Status:** ready-for-agent

- [ ] Silence/noise canary fixtures produce zero blocklist phrases ("thank you for watching" family, "you"/"♪" drops) — the zh-CN blocklist gets its first entries from fixture runs
- [ ] VAD masking on the mic channel (zero frames, never drop — timeline preserved), pre-decode energy gate, `[_BEG_]` logits pin, rolling initial-prompt, repetition-ratio whole-result drop
- [ ] Caption partials are prefix-stable (a partial never visibly retracts committed words in the stabilizer test); wall-clock drift >3s injects a gap-skip so timestamps never desync
- [ ] Queued-vs-completed accounting: clean stop reports zero lost chunks; a forced loss emits the chunk-loss event
- [ ] Killing the ASR leg mid-recording: capture and AAC writing continue, a degraded-state event reaches clients, the Meeting finalizes with its partial transcript flagged — recording never depends on ASR health
