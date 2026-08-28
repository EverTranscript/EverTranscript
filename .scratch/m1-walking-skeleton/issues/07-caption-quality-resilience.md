# 07: Caption quality + resilience kit

**What to build:** The transcript becomes trustworthy under adverse conditions: the layered hallucination defenses, the prefix-commit caption stabilizer with its gap-skip clock, tail-integrity accounting so a stop never silently loses the last sentence, and the ADR-0029 degradation contract — a dead ASR leg never stops the recording.

**Blocked by:** 04, 06.

**Status:** done — partial captions and VAD masking are recorded non-goals, with reasons

- [x] Silence/noise canary fixtures produce zero blocklist phrases ("thank you for watching" family, "you"/"♪" drops) — the zh-CN blocklist gets its first entries from fixture runs
- [~] Pre-decode energy gate, rolling initial-prompt, and repetition-ratio whole-result drop are done.
      **Not done:** VAD masking (the chunker already gates silence, so masking's marginal value is low and its
      risk — zeroing quiet real speech — is real) and the `[_BEG_]` logits pin (whisper-rs 0.16 exposes no
      logits-filter hook; `no_speech_thold` and the filters cover the same failure).
- [~] **Not done, and now a deliberate non-goal rather than an omission.** The chunker emits finished utterances
      only, so there is nothing to stabilize and no prefix-commit is needed; timestamps cannot desync because they
      come from the capture clock rather than wall time (ADR-0029). The question was whether the latency justifies
      the risk, and the numbers say no: the adaptive negative threshold relaxes from 0.80 to 0.35 between 3s and
      20s, so a chunk closes at the first natural pause once it has been running — 3–6s in ordinary speech, with
      the 25s cap reached only by genuinely unbroken monologue. Against that, transcribing incomplete utterances is
      precisely the condition that produces the inventions ticket 07 exists to suppress. Revisit if dogfooding
      shows the wait actually feels dead.
- [~] Transcription is synchronous within the recorder, so there is no queue to lose chunks from. The property
      that mattered — the trailing utterance surviving stop — is tested directly instead.
- [x] Killing the ASR leg mid-recording: capture and AAC writing continue, a degraded-state event reaches clients, the Meeting finalizes with its partial transcript flagged — recording never depends on ASR health
