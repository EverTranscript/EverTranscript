# The Summary model becomes Qwen3-4B, and arrives on its own

Status: ready-for-agent

Synthesized from a grilled design session (2026-09-01) covering thirty-six decisions,
every one put to the Operator and answered. Glossary terms per `CONTEXT.md` — **Provisioned
Model** and the amended **Sanctioned Traffic** entry were added to it during that session.

Four ADRs are amended by this work: 0002, 0013, 0034, and the Briefing text ADR-0007
governs. Untouched and load-bearing: ADR-0009 (immutable record), ADR-0019 (degradation
never costs the recording), ADR-0026 (the Core is the only writer), ADR-0028 (additive-only).

## Problem Statement

The Summary an Operator gets is bad, and the registry says so in its own comment: the
shipped model is *"the model that was verified, not the model that should ship."*
DECISIONS Q45 measured it on three lines containing one unmistakable commitment. It
answered `None noted.`, then contradicted itself with four `Who | What | When | Said at`
rows, then reproduced all three transcript lines verbatim — and the `Said at` times it
gave (`14:00`, `12:30`) and the `When` values (`Monday`, `Thursday morning`) appear
nowhere in the input. The column whose stated job is letting an item be checked against
what was said is the column being invented, inside a record ADR-0009 makes permanent.

It is also invisible. Summary's model is the one thing a working install may lack, so an
Operator can reach the feature, find it unavailable, and never learn why.

## Solution

Replace the model with Qwen3-4B (UD-Q4_K_XL), teach the sidecar to drive it the way its
publisher documents, and make it arrive by itself so the feature is there when reached.

Three of those are one idea: a model is not just a file. How it wants to be framed, how it
wants to be sampled, whether it reasons aloud, and how much context it can take are facts
about *that model*, and they belong beside its checksum rather than hardcoded in the
sidecar. Making them entry properties is what lets this swap be a data change plus a seam,
instead of a rewrite that the next swap repeats.

The merge is gated on measurement. A bigger model that still fabricates timestamps is not
an improvement worth 2.5 GB, and "it seems better" is what an undefined bar collapses into.

## User Stories

1. As an Operator, I want a Summary that finds the commitments people actually made, so that the record is worth reading.
2. As an Operator, I want a Summary that never invents a timestamp, so that the one column meant for checking claims cannot itself be a false claim.
3. As an Operator, I want the Summary model present when I first reach the feature, so that discovering Summary is not the same moment as discovering I must download something.
4. As an Operator on a fresh install, I want to be told what will be downloaded before it happens, so that a large transfer is never a surprise.
5. As an Operator, I want to see the download progressing and be able to stop it, so that a background transfer is never something I merely suspect is happening.
6. As an Operator, I want the download to survive quitting the app, so that a slow connection is an inconvenience rather than a restart.
7. As an Operator with a nearly full disk, I want to be told before a 2.5 GB fetch begins, so that I learn at the start rather than at ninety percent.
8. As an Operator, I want my first recording to work while the model is still arriving, so that setting the product up and using it are not mutually exclusive.
9. As an Operator, I want a clear reason when Summary is unavailable — still downloading and how far along, absent, unable to start, or too large for this machine — so that I know whether to wait, retry, or stop expecting it.
10. As an Operator upgrading, I do not want a multi-gigabyte download to begin unannounced, so that installing a newer version of what I had stays a small act.
11. As an Operator upgrading, I want the model I no longer use removed, so that half a gigabyte is not stranded on my disk forever.
12. As an Operator, I want Local preselected, so that a working configuration exists without me having to research a choice I have no basis for.
13. As an Operator, I want choosing Cloud to remain a deliberate act with its warning intact, so that the one path that sends my meetings somewhere is never the path I drifted into.
14. As an Operator who edits the system prompt, I want my prompt to be about summaries, so that model-specific incantations are not mine to maintain.
15. As an Operator who pastes a system prompt from elsewhere, I do not want its stray characters to break the conversation structure, so that the armor protecting Notes protects this too.
16. As an Operator, I want a long meeting summarized in fewer, larger passes, so that a bigger model's context is spent on the meeting rather than on re-reading overlaps.
17. As an Operator, I do not want a generation to time out on a real ninety-minute meeting, so that the bound protects me from a wedged sidecar rather than from finishing.
18. As an Operator, I want the Summary not to think out loud, so that I am not waiting on reasoning that gets discarded before I see it.
19. As an Operator on a modest machine, I want the model loaded in a way that fits, so that Summary degrades rather than taking the machine down with it.
20. As an Operator, I want Summary generation never to slow a recording in progress, so that the feature that runs afterwards cannot damage the thing it describes.
21. As a maintainer, I want every model's licence and source recorded, so that a public Apache-2.0 project can say where its artifacts came from.
22. As a maintainer, I want the guarantee test to keep proving silence, so that adding an automatic download does not quietly hollow out the strongest claim this product makes.
23. As a maintainer, I want proof that provisioning actually fires, so that the flag being right and the flag being read are not confused.
24. As a maintainer, I want the model swap gated on a measurement with a named bar, so that the next swap inherits a standard rather than an anecdote.
25. As a maintainer, I want CI's model fetch cached, so that a 2.5 GB download per platform per push is not the dominant cost of a run.
26. As a reader of the Briefing, I want it to distinguish what leaves the machine from what the machine contacts, so that "local models" is not mistaken for "silent".

## Implementation Decisions

### The model

- Replace `SUMMARY_DEFAULT` with `Qwen3-4B-UD-Q4_K_XL.gguf` from `unsloth/Qwen3-4B-GGUF`:
  **2,546,341,152 bytes**, sha256 **`f6e3fb6c2cdc869d16e66c719e94f2c02095d195967230e759a2d77fe814c71f`**,
  Apache-2.0, public and ungated. Local filename follows the existing `summary-` convention.
  The 0.5B is not kept as an alternative — two models means two quality stories and a
  support surface where "my summaries are bad" has an invisible cause.
- **Verify against the LFS sha256, not the CDN etag.** The CDN returns a Xet content hash
  that is not the sha256, and the plain `Q4_K_M` variant's sha256 also begins `f6` — so
  compare full strings. Both traps cost a failing install for an invisible reason.
- Every registry entry gains **licence and source**. Not just the new one: four models
  currently record no provenance in a public Apache-2.0 project that keeps a careful
  ledger for ported source.

### Prefactor: the sidecar's resource claim is currently false

M4's `04-local-sidecar` states the sidecar is *"spawned at reduced priority and with a
layers-that-fit calculation."* Neither exists — the load is `LlamaModelParams::default()`.
Implement both and un-tick the criterion until it is true. This lands **before** the swap:
a 4B with every layer offloaded by default is what makes the omission bite. Note the
constraint — `with_n_gpu_layers` takes `u32` while the default `-1` means *all layers*, so
once a number is set, "all" is no longer expressible through that setter.

### A model is described, not hardcoded

Four things move onto the registry entry, because they are facts about a model rather than
constants of the product:

- **Framing.** Read the GGUF's embedded chat template and apply it. Raw framing stays
  expressible for models without one — `apply_chat_template` has no ChatML fallback.
- **Sampling.** Qwen3's documented non-thinking settings: temperature 0.7, top-p 0.8,
  top-k 20, min-p 0, chain ending in a distribution sampler. The card says outright *"DO
  NOT use greedy decoding"*, and the sidecar's own comment records that greedy is why a
  repetition penalty was needed.
- **Thinking suppression.** Appended by the sidecar, not placed in `DEFAULT_SYSTEM_PROMPT`
  — the system prompt is Operator-editable, and an Operator rewriting it would silently
  re-enable thinking with no visible cause. The hard switch is a template variable this
  API cannot reach, so the soft switch is the mechanism.
- **Context budget.** Read the model's own trained context rather than hardcoding.
  `CONTEXT_TOKENS` rises to 16,384 and the single-pass threshold to ~12,000 — not the full
  40,960, because KV cache costs most exactly when Summary competes with a recording.

**The Operator's system prompt is now escaped**, as Notes already are. Under raw framing a
stray `<|im_end|>` was characters; under a chat template it terminates a turn. The existing
justification applies verbatim: the Operator is trusted, text they pasted from elsewhere is
not necessarily.

**Stop sequences are reviewed against the new framing** — `"\nSummary:"` guards a suffix
that disappears with the template, and the real terminator becomes the model's EOS, which
the sidecar already honours. Keep only what can still occur; `</transcript>` earned its
place by an observed replay.

**`REQUEST_TIMEOUT` is re-derived from the measurement.** Its comment says it was sized for
"a small model"; we are deleting the small model and tripling per-chunk context, and Q46
only just made it enforced rather than decorative. A bound calibrated for the model being
removed, newly enforced, is a timeout that will fire on real meetings.

**`estimate_tokens`** (0.35 tokens per character) was never calibrated against a tokenizer.
At a 4,000-token budget the error was small; at 12,000 it is three times larger in absolute
terms. Revisit against Qwen3's actual tokenizer.

### The model arrives on its own

- `required: true`, fetched **automatically on a fresh install only**. An upgrade that
  introduces a new required model asks once — a fresh install consented to setting the
  product up; an upgrade consented to a newer version of what it had.
- Background, resumable, never fatal. A recording is never held hostage; ADR-0019 already
  says a missing model costs the feature and not the record.
- **No pause for recordings.** Recording never touches the network, so a download cannot
  contend for the resource it uses; disk and a little CPU are the only overlap.
- The **total size is stated before it begins**.
- **Progress and cancellation are visible**, mirroring the existing diarization-progress
  notification. Both additive per ADR-0028. An automatic multi-gigabyte transfer with no
  indicator and no stop is indistinguishable, from outside, from the product misbehaving.
- **Disk space is checked before starting**, not discovered at ninety percent.
- **Provisioning is requested, not implicit in construction.** A Core built by a test is
  not a fresh install; the guarantee tests build fresh Cores against isolated Application
  Support directories and would otherwise begin downloading inside the test that exists to
  prove no sockets open. Suppressing that with a test-only switch would leave the flagship
  guarantee proven only with the new behaviour disabled.

### Defaults

- **Local is preselected** and written to settings on first Core start, so a stored
  configuration is always explicit. Never clobber an existing choice — an Operator who
  chose Cloud must not be reset to Local by an upgrade.
- Choosing Cloud keeps its hard one-time warning. What changes is preselection, for the
  Summary Backend only.
- When Summary is invoked before the model has landed, it **fails and can be retried** —
  no queued generation, which would start minutes of CPU at an unpredictable moment.

### Failures name their cause

`local()` currently collapses missing, won't-start, and load-failure into *"the local
Summary model is not available."* Four causes are now distinguished, because the Operator's
action differs completely between them: **still downloading, with progress** (the most
likely first-run case, and the `Partial { bytes_on_disk }` state already exists to report
it); **absent**; **would not start**; **would not fit**. "Not available" for a model that is
on disk and simply too large is the Q47 mistake — true, and useless.

### Cleanup

The orphaned `summary-qwen2.5-0.5b-instruct-q4_k_m.gguf` is deleted on the upgrade that
replaces it — **by exact filename, never a glob**, so a stray file of the Operator's is
never swept up. Application Support is the re-creatable half; History is never touched.

### Documents that stop being true

- **The Briefing** says *"model downloads you trigger."* False under automatic fetching.
  Reworded — and per the session's own evidence, the new sentence must separate *content*
  from *wire*: choosing Local means no meeting content ever leaves, **and** the product
  still makes network calls. This is the consent gate, legal copy under ADR-0007, with an
  open M5 criterion for counsel. It must be *true* before counsel sees it; counsel judges
  sufficiency, not accuracy.
- **ADR-0002** — Summary's model becomes provisioned. Summary remains non-Anchor: it still
  has a Knob. Provisioning and anchoring are different properties, which is why the
  glossary now has a word for the second.
- **ADR-0013** — amended with scope explicit: preselection applies to the Summary Backend
  only, the cloud warning stays a hard gate, ADR-0007's recording-start property untouched.
  The amendment should own that the Core now writes a setting nobody typed, which is
  precisely the property 0013 named.
- **ADR-0034** — entry two says downloads happen "at explicit moments"; amended.
- **`CONTEXT.md`** — already done in-session: Sanctioned Traffic notes that downloads are
  not all Operator-initiated, and **Provisioned Model** is defined against Anchor.

### CI

Cache the model fetch **keyed on its sha256**, so a changed model is a different key rather
than a stale hit. At 2.5 GB per platform per push, uncached is the dominant cost of a run
and a new flakiness surface. Keep the inference test on both platforms — Q45 exists
precisely because "it cross-compiles" had been standing in for "it runs".

## Testing Decisions

Good tests here assert what an Operator or maintainer can observe — which Backend answered,
what the record holds, what the failure said, whether a socket opened — never the internal
call order. Prior art is the pattern this repo reached for twice in the Electron work:
extract the judgement, test it purely, then one integration test proves the wiring.

- **Provisioning decision** — a pure "should this Core provision, given its state?" tested
  directly, plus one end-to-end test against a local stub via the existing
  `Downloader::with_base_url` seam (prior art: `model_download.rs`). Neither touches the
  internet. Decision-only would repeat Q44, where the logic was sound and nothing ran it.
- **The guarantee tests keep their meaning** — the existing steady-state assertion stands,
  and a fresh Core must not fetch inside it.
- **Failure taxonomy** — each of the four causes produces its own message, driven through
  the existing Backend-factory seam with no model.
- **Registry data** — licence, source, size, checksum and the new per-model properties are
  data and can be asserted without loading anything.
- **Quality is a separate, model-gated test whose subject is the model.** It asserts
  **zero fabricated timestamps** — a gate, not a score, because inventing a `Said at` puts
  a false claim into a record ADR-0009 makes permanent — and requires improvement on
  action-items-found and verbatim-echo against Q45's recorded numbers (0 of 1 found, 3 of 3
  lines echoed). `summary_inference` stays the platform test and keeps *reporting* rather
  than asserting; the separation ticket 03 drew is deliberate.
- **The measurement gates the merge.** The swap does not land until the numbers are in, so
  there is nothing to roll back and no Operator's disk has lost the old model before we
  knew. The re-derived `REQUEST_TIMEOUT` is read off the same run.

## Out of Scope

- A lighter registered alternative for modest machines. If wanted, it follows a
  measurement rather than preceding one.
- Changing the update check's default, or making downloads Operator-triggered again —
  both were considered and declined; the guarantee stays conditional and the Briefing
  says so.
- YaRN and any context beyond the model's native 32,768.
- Reworking the Knob's fallback direction, the cloud warning, or the Closed Boundary.
- The long-meeting Summary quality measurement M4 still owes — this makes chunking
  engage and re-measures the short case; ninety minutes through a real Backend remains open.
- Bilingual transcription quality (CER 70.7%), tracked separately.

## Further Notes

- **Record against M5's Briefing criterion**, with limits stated: during this session the
  Operator asked three times whether local models mean zero traffic outside. The Briefing's
  final section addresses exactly that and is accurate. They are not a stranger and were
  not reading it at the time, so it is weak evidence — but *local inference implies
  silence* is the most natural wrong conclusion a privacy-minded reader can reach, and it
  was reached. It does not make the criterion met; it should shape the rewording.
- Journal the consequential calls: automatic provisioning (a Nothing-Ambient-adjacent
  reversal argued against and overruled), Local preselection (spends ADR-0013's stated
  property), and the false M4 resource criterion. **Re-read the journal tail immediately
  before appending** — a concurrent session has produced a duplicate Q-number in this repo
  once already.
- Existing Meetings keep the Backend label naming the old model. That is the record being
  honest about what produced it, and must not be migrated.
