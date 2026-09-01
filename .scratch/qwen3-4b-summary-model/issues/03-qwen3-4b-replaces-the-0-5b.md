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

**Status:** ready-for-agent

- [ ] The registry names the new model with its verified size, sha256, licence and source
- [ ] The sidecar applies the model's embedded chat template, and its prescribed sampling replaces greedy decoding
- [ ] Reasoning is suppressed by the sidecar, not by the Operator-editable system prompt — an Operator who rewrites their prompt must not silently re-enable it
- [ ] The Operator's system prompt is escaped the way Notes already are, because a chat template turns stray markers into turn boundaries
- [ ] Stop sequences are reviewed against the new framing; those guarding text the prompt no longer emits are removed, and the comments describe what is actually there
- [ ] The single-pass threshold and context budget use the model's own trained context rather than the numbers sized for a 0.5B
- [ ] The token estimate is re-checked against this model's tokenizer, since the budget it feeds is now three times larger
- [ ] A Summary is generated end to end by the new model
- [ ] The local gate is green
