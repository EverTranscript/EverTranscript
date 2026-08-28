# 07: Caption quality + resilience kit

**What to build:** The transcript becomes trustworthy under adverse conditions: the layered hallucination defenses, the prefix-commit caption stabilizer with its gap-skip clock, tail-integrity accounting so a stop never silently loses the last sentence, and the ADR-0029 degradation contract — a dead ASR leg never stops the recording.

**Blocked by:** 04, 06.

**Status:** mostly done — partial captions and VAD masking outstanding

- [x] Silence/noise canary fixtures produce zero blocklist phrases ("thank you for watching" family, "you"/"♪" drops) — the zh-CN blocklist gets its first entries from fixture runs
- [~] Pre-decode energy gate, rolling initial-prompt, and repetition-ratio whole-result drop are done.
      **Not done:** VAD masking (the chunker already gates silence, so masking's marginal value is low and its
      risk — zeroing quiet real speech — is real) and the `[_BEG_]` logits pin (whisper-rs 0.16 exposes no
      logits-filter hook; `no_speech_thold` and the filters cover the same failure).
- [~] **Not done: partial captions do not exist yet.** The chunker emits finished utterances only, so there is
      nothing to stabilize and no prefix-commit is needed. Timestamps cannot desync because they come from the
      capture clock rather than from wall time (ADR-0029), which makes the gap-skip clock unnecessary here.
      Adding partials is what would make this criterion live.
- [~] Transcription is synchronous within the recorder, so there is no queue to lose chunks from. The property
      that mattered — the trailing utterance surviving stop — is tested directly instead.
- [x] Killing the ASR leg mid-recording: capture and AAC writing continue, a degraded-state event reaches clients, the Meeting finalizes with its partial transcript flagged — recording never depends on ASR health
