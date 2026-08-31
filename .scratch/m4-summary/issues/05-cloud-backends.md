# 05: Cloud Backends — OpenAI-compatible, labeled, never gated

**What to build:** The other side of the Knob, and the only path in this product that may carry meeting content over the network.

**Blocked by:** 01.

Status: not started

- [ ] An OpenAI-compatible client — the abstraction ADR-0031 already assumes, so Ollama, LM Studio and every cloud preset are one implementation with different base URLs
- [ ] **Curated presets carry verified data-handling labels** (ADR-0010): trains on inputs? retention window? ZDR available? Verified at release time, with the verification date visible, because an unverifiable label is worse than none
- [ ] **Labels inform and never gate** (ADR-0010). The product cannot verify provider-side retention, so a ZDR-only gate would be false hardness dressed as a guarantee. The custom base-URL field stays fully open, labeled "unknown endpoint — your rules"
- [ ] Errors map onto the seam's shapes (01) — unreachable, refused, timed out, malformed — so the fallback policy never has to parse a provider's prose
- [ ] Requests carry the transcript and nothing else about the machine: no identifiers, no telemetry, no meeting metadata the Summary does not need. This is the single largest exfiltration surface in the product and its payload should be inspectable in one screenful of code
- [ ] **This is Sanctioned Traffic entry three and only entry three** (ADR-0034). It is reachable only when the Operator chose Cloud, and the zero-network guarantee test must still pass with the Knob on Local
- [ ] Both platforms (ADR-0025 as amended)
