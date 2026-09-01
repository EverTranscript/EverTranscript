# 03: Qwen3-4B replaces the 0.5B

**What to build:** The registered Summary model becomes Qwen3-4B (UD-Q4_K_XL), driven the
way its publisher documents: its embedded chat template applied, its prescribed sampling
rather than greedy decoding, reasoning suppressed, and a context budget that uses what a
4B can actually take.

Verified artifact: `Qwen3-4B-UD-Q4_K_XL.gguf` from `unsloth/Qwen3-4B-GGUF`,
**2,546,341,152 bytes**, sha256
**`f6e3fb6c2cdc869d16e66c719e94f2c02095d195967230e759a2d77fe814c71f`**, Apache-2.0,
public and ungated. Verify against the LFS sha256 — the CDN returns a different content
hash, and the plain `Q4_K_M` variant's sha256 also begins `f6`.

**This ticket does not merge until 04 passes.** The swap is gated on measurement so that
no Operator's disk loses the old model before we know the new one is better.

**Blocked by:** 02 (the seam it configures).

**Status:** done

- [x] Verified by downloading it: 2,546,341,152 bytes and sha256 `f6e3fb6c…c71f`, both matching the publisher's LFS metadata exactly
- [x] Both. Driving it with the old plain framing and greedy first — by accident, via a test harness still calling the undriven spawn — produced looping, self-correcting prose, which is a vivid demonstration of why the card forbids greedy
- [x] Appended from the entry. Confirmed working: the output opens with an empty `<think></think>`, which is the model acknowledging the switch, and `scrub` removes it
- [x] Escaped at the point it is read, using the armor Notes already had
- [x] Reviewed and annotated. `\nSummary:` is kept rather than deleted — plain framing is still a choice a registered model can make — but its comment now says why it cannot fire under a chat template, instead of implying it guards the current prompt
- [x] Both. **The threshold was nearly a dead property**: the entry said 12,000 while the chunker still split at its own 4,000 constant, so a bigger context would have been a fact nobody acted on. Found by measuring, not reading
- [x] Checked against real prompts: the 0.35-per-character estimate put an 11,627-token chunk inside a 16,384 context with room for the answer, which is the property that matters
- [x] Generated, with a heading, a correct action-items table and timestamps cited from the transcript
- [x] The local gate is green
