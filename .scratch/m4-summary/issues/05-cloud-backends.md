# 05: Cloud Backends — OpenAI-compatible, labeled, never gated

**What to build:** The other side of the Knob, and the only path in this product that may carry meeting content over the network.

**Blocked by:** 01.

Status: done, with the labels honestly marked unverified

- [x] An OpenAI-compatible client — the abstraction ADR-0031 already assumes, so Ollama, LM Studio and every cloud preset are one implementation with different base URLs
- [ ] **The structure ships; the verification has not happened.** Each cloud preset carries the three fields and a `verified_on` date, and that date reads `unverified` — because ADR-0010 requires a human to have read the provider's terms at release time and nobody has. Writing plausible values with a plausible date would be exactly the false assurance the ADR forbids, so there is a test asserting the labels still admit they are unverified. It will fail the day someone fills them in, which is when they should also delete it
- [x] **Labels inform and never gate** (ADR-0010). The product cannot verify provider-side retention, so a ZDR-only gate would be false hardness dressed as a guarantee. The custom base-URL field stays fully open, labeled "unknown endpoint — your rules"
- [x] Mapped, and the error **body is deliberately not included** in what is reported: it can carry a key, an org id, or an echo of the prompt, and an error logged verbatim is a way for secrets to reach a log file. Only the status code
- [x] Two messages and a model name, constructed in one place. Nothing about the machine, the Operator, or the Meeting beyond the text being summarized — everything sent is something the Operator could have pasted themselves
- [x] **This is Sanctioned Traffic entry three and only entry three** (ADR-0034). It is reachable only when the Operator chose Cloud, and the zero-network guarantee test must still pass with the Knob on Local
- [x] Both platforms (ADR-0025 as amended)

## 2026-09-05 — the legwork, separated from the signature

The open criterion above bundles two different things: reading four
provider pages, and a person deciding they believe them. Only the second
needs a person, so the first was done and written down —
`docs/provider-terms-2026-09-05.md` carries the quotes, the links, the
proposed field values, and the exact four-step edit that closes this
criterion.

Two things changed in the code as a side effect of reading those pages,
neither of which needs a sign-off:

- The `anthropic` preset defaulted to `claude-sonnet-4-5`, which retires no
  sooner than 2026-09-29. Since this client reports only a status code and
  never an error body, that retirement would have reached the Operator as
  the bare string `HTTP 404`. Now `claude-opus-5`.
- The preset reaches Anthropic through their OpenAI-compatibility layer,
  which Anthropic themselves describe as not a production path. It is still
  the right call — the alternative is a second exfiltration surface — but
  it is now written down beside the preset instead of being invisible.

The `openai` preset's `gpt-4o-mini` was deliberately left alone: not
deprecated, no shutdown date, so changing it is a product judgement about
defaults rather than a defect, and it belongs to whoever owns the picker.
