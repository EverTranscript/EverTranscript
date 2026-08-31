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
