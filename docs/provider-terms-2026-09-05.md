# Cloud provider data-handling terms — read 2026-09-05

**Status: read and signed off 2026-09-05.** ADR-0010 requires that a *human*
has read each provider's terms at release time. The quotes and links below
are what was read; the Operator read them and accepted them, and
`summary/cloud.rs` now carries `verified_on: "2026-09-05"` instead of
`unverified`.

**This file is the evidence behind a claim the product makes on screen** —
the Client prints `Verified: 2026-09-05` beside each cloud Backend. Terms
change without announcement, so the date is the mechanism, not decoration:
re-read all four sources before each release and write a *new* dated file
rather than editing this one, exactly as `competitive-facts-*.md` does.

## What this product actually sends

Worth establishing first, because it narrows which clauses apply. Every
cloud request is built in one place (`CloudBackend::generate`) and carries
a model name, one system message, and one user message. No `store`, no
`user`, no `metadata`, no feedback signal, no identifiers, no meeting
metadata. So provider clauses conditioned on opting in, on submitting
feedback, or on server-side conversation storage cannot be triggered by
this product — they are listed below anyway, because "cannot be triggered
today" is a claim about our code that a future change could quietly break.

## OpenAI

Source: <https://developers.openai.com/api/docs/guides/your-data>

- **Training.** "Data sent to the OpenAI API is not used to train or improve
  OpenAI models (unless you explicitly opt in to share data with us)." The
  page dates this policy to March 1, 2023.
- **Retention.** "Abuse monitoring logs are generated for all API feature
  usage and retained for up to 30 days, unless longer retention is required
  by law." Separately, *application state* retention varies by endpoint,
  from none to stored-until-deleted — that is the `store` / stateful-endpoint
  family, which this product does not use.
- **Zero retention.** Available but not self-serve: "Eligible customers may
  have their customer content excluded from these abuse monitoring logs …
  by getting approved for the Zero Data Retention … controls", which is
  "subject to prior approval by OpenAI and acceptance of additional
  requirements." Arranged through their sales team.

Label as shipped: `trains_on_inputs: false`, `retention: "30 days
(abuse-monitoring logs)"`, `zero_retention_available: true`.

The `retention` string was the one that most wanted a human's eye. "30 days"
alone would be a slightly flattering summary of a page that also describes
endpoints retaining until deletion; the parenthetical is there so the label
says which 30 days it means. `zero_retention_available: true` is likewise
true-but-not-easy — approval by OpenAI plus additional terms — and the field
asks whether ZDR is available, not whether it is convenient.

## Anthropic

Sources: <https://privacy.claude.com/en/articles/7996866-how-long-do-you-store-my-data>
(the page reports last-updated 2026-07-01) and
<https://privacy.claude.com/en/articles/7996868-is-my-data-used-for-model-training>

- **Training.** "By default, we will not use your inputs or outputs from our
  commercial products *(e.g. Claude for Work, Anthropic API, Claude Gov,
  etc.)* to train our models."
- **Retention.** "For Anthropic API users, we automatically delete inputs and
  outputs on our backend within 30 days of receipt or generation, except…",
  the exceptions including having "agreed otherwise (e.g. zero data
  retention agreement)".
- **The feedback exception.** If a user submits thumbs-up/down feedback or
  otherwise opts in, the related conversation may be stored up to five years
  and used for training, de-linked from user and customer IDs. **This
  product sends no feedback signal and has no surface that could**, so the
  exception is unreachable from here — see "What this product actually
  sends" above.
- **Zero retention.** Available by agreement.

Label as shipped: `trains_on_inputs: false`, `retention: "30 days"`,
`zero_retention_available: true`.

## The Anthropic preset is an OpenAI-compatibility layer, and Anthropic hedges it

Source: <https://platform.claude.com/docs/en/api/openai-sdk>

The `anthropic` preset works because Anthropic serves an OpenAI-shaped
endpoint at `https://api.anthropic.com/v1/` — `POST /v1/chat/completions`,
with `authorization` listed as "Fully supported", so the bearer token this
client already sends is correct and no `anthropic-version` header is
needed. Two details land in our favour: multiple system messages are
hoisted and concatenated (we send exactly one), and the fields the layer
silently ignores — `response_format`, `store`, `user`, `metadata`, `seed` —
are all fields we do not send.

The caveat is Anthropic's own, and it belongs in a maintainer's field of
view rather than only in a doc: the layer "is primarily intended to test and
compare model capabilities, and is not considered a long-term or
production-ready solution for most use cases", though it "is intended to
remain fully functional and not have breaking changes". Reaching Anthropic
through their native `/v1/messages` API would mean a second Backend
implementation and a second exfiltration surface to audit — which ADR-0031's
one-client design deliberately refused. Keeping the compatibility layer is
still the right call; knowing it is a compatibility layer is the point.

## Model IDs

Source: <https://platform.claude.com/docs/en/about-claude/model-deprecations>

The `anthropic` preset shipped `claude-sonnet-4-5`. The deprecations table
lists `claude-sonnet-4-5-20250929` as Active with a tentative retirement
"Not sooner than September 29, 2026" — twenty-four days after this file was
written — and does not list the bare `claude-sonnet-4-5` alias at all. A
retired model returns an HTTP error, and this client deliberately reports
only the status code (the body can carry a key or a prompt echo), so the
Operator's experience of that retirement would be the word `HTTP 404` and
nothing else. Changed to `claude-opus-5`, which the same table lists Active
with retirement not sooner than July 24, 2027.

Active Claude model IDs as of this date: `claude-fable-5-1`,
`claude-fable-5`, `claude-opus-5`, `claude-opus-4-8`, `claude-opus-4-7`,
`claude-opus-4-6`, `claude-opus-4-5-20251101`, `claude-sonnet-5`,
`claude-sonnet-4-6`, `claude-sonnet-4-5-20250929`,
`claude-haiku-4-5-20251001`.

The `openai` preset shipped `gpt-4o-mini`, which is **not** deprecated and
carries no announced shutdown date — so unlike the Anthropic case this was a
defaults judgement rather than a defect. It was two generations old, and an
Operator who chose Cloud over the bundled local Qwen3-4B could plausibly
have landed on a *worse* Summary than the one they turned down. Changed to
`gpt-5.6-luna`, OpenAI's current cost-optimised model — the closest thing to
what `gpt-4o-mini` was when it was first chosen.

OpenAI's current chat models: `gpt-6-astra` ("our most capable"),
`gpt-5.6-sol` (general purpose), `gpt-5.6-terra` (latency-sensitive), and
`gpt-5.6-luna` (cost-optimised)
(<https://developers.openai.com/api/docs/models>).

## What was done on sign-off

1. The two `retention` strings filled in
   (`crates/evertranscript-core/src/summary/cloud.rs`).
2. Both `verified_on` fields set to `2026-09-05`.
3. `the_labels_admit_they_are_unverified` deleted — it existed to fail at
   exactly this moment, and keeping it would have been the lie in the other
   direction. `every_label_carries_a_real_date` replaces it: the Client
   prints this string verbatim beside the word "Verified", so free text
   there becomes a lie on screen.
4. The criterion ticked in `.scratch/m4-summary/issues/05-cloud-backends.md`.

## Owed at the next release

Re-read all four sources and write `docs/provider-terms-<date>.md`, then
re-date the `verified_on` fields. A label whose date has gone stale is the
failure mode this whole mechanism exists to make visible — the Client shows
the date to the Operator precisely so that a year-old claim looks like a
year-old claim.

One known rough edge, deliberately not fixed: `retention` is rendered raw
and is English only, so the Chinese locale shows an English phrase. It is a
quote of the provider's own English terms, and translating a legal
characterisation is a worse risk than leaving it legible.
