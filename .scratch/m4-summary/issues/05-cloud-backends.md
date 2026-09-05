# 05: Cloud Backends — OpenAI-compatible, labeled, never gated

**What to build:** The other side of the Knob, and the only path in this product that may carry meeting content over the network.

**Blocked by:** 01.

Status: done, labels verified 2026-09-05

- [x] An OpenAI-compatible client — the abstraction ADR-0031 already assumes, so Ollama, LM Studio and every cloud preset are one implementation with different base URLs
- [x] **The structure ships, and as of 2026-09-05 so does the verification.** Each cloud preset carries the three fields and a `verified_on` date, and that date reads `unverified` — because ADR-0010 requires a human to have read the provider's terms at release time and nobody has. Writing plausible values with a plausible date would be exactly the false assurance the ADR forbids, so there is a test asserting the labels still admit they are unverified. It will fail the day someone fills them in, which is when they should also delete it
- [x] **Labels inform and never gate** (ADR-0010). The product cannot verify provider-side retention, so a ZDR-only gate would be false hardness dressed as a guarantee. The custom base-URL field stays fully open, labeled "unknown endpoint — your rules"
- [x] Mapped, and the error **body is deliberately not included** in what is reported: it can carry a key, an org id, or an echo of the prompt, and an error logged verbatim is a way for secrets to reach a log file. Only the status code
- [x] Two messages and a model name, constructed in one place. Nothing about the machine, the Operator, or the Meeting beyond the text being summarized — everything sent is something the Operator could have pasted themselves
- [x] **This is Sanctioned Traffic entry three and only entry three** (ADR-0034). It is reachable only when the Operator chose Cloud, and the zero-network guarantee test must still pass with the Knob on Local
- [x] Both platforms (ADR-0025 as amended)

## 2026-09-05 — closed

The criterion bundled two things: reading four provider pages, and someone
deciding they believe them. Only the second needed a person, so the reading
was done first and written down — `docs/provider-terms-2026-09-05.md` has
the quotes, the links and the sources. The Operator then read it and signed
off, which is what ADR-0010 was actually asking for and what no amount of
fetching could supply.

The labels now read: both providers no-training-by-default, OpenAI 30 days
(abuse-monitoring logs) and Anthropic 30 days, ZDR available on both by
approval, `verified_on: "2026-09-05"`.

`the_labels_admit_they_are_unverified` is deleted, as the criterion said it
should be. `every_label_carries_a_real_date` takes its place — not the same
test wearing a new name, but the invariant that outlives it: the Client
prints this string verbatim beside the word "Verified", so free text there
becomes a lie on screen, and a claim with no date cannot be known to be
stale.

Two things also changed as a side effect of reading those pages:

- The `anthropic` preset defaulted to `claude-sonnet-4-5`, which retires no
  sooner than 2026-09-29. Since this client reports only a status code and
  never an error body, that retirement would have reached the Operator as
  the bare string `HTTP 404`. Now `claude-opus-5`.
- The preset reaches Anthropic through their OpenAI-compatibility layer,
  which Anthropic themselves describe as not a production path. Still the
  right call — the alternative is a second exfiltration surface — but it is
  now written down beside the preset instead of being invisible.

And `gpt-4o-mini` became `gpt-5.6-luna`: not a defect (the old model is
still served) but a defaults judgement the Operator made, on the grounds
that someone who chose Cloud over the bundled local Qwen3-4B should not
land on a smaller, older model than the one they turned down.

**What is now owed, and is not a defect:** these labels are dated because
terms change. Re-read the four sources before each release and re-date both
the file and the fields; the date is the whole mechanism.
