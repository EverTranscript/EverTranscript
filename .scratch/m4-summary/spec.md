# M4 — Summary, Notes, and the Knob: the one place cloud is ever allowed

Status: ready-for-agent

Sources of truth: `CONTEXT.md` (glossary — normative), `docs/prd.md` (stories 34–42), ADR-0031 (local Summary is a bundled llama.cpp sidecar), ADR-0013 (the Backend picker forces an explicit choice), ADR-0010 (presets are labeled, never gated), ADR-0018 (Operator Notes steer Summary), ADR-0034 (Sanctioned Traffic), ADR-0003 (the Knob machinery), ADR-0002 (Transcription and Diarization are Anchors — Summary is not), `docs/implementation-notes-2026-08-27.md` (the absorption catalog — its M4 section carries an operational constant for nearly every decision below). Where this spec and an ADR disagree, the ADR wins.

## Problem Statement

Three milestones have built a record: captured, attributed, searchable, and entirely local. Nobody reads it. A ninety-minute meeting produces a ninety-minute transcript, and the thing an Operator actually wants — what was decided, what they agreed to do — is still buried in it. `## Summary` and `## Notes` have been headings in every Mirror since M1 with `*Not generated yet.*` underneath.

This is also the milestone where the product does the thing it has spent three milestones promising not to do: **send meeting content over the network**. ADR-0034 permits exactly one such path — a cloud Summary Backend the Operator explicitly chose — and everything about how that choice is made, stored, indicated, and recovered from is what this milestone is really about. A Summary feature with a careless Knob would undo the guarantee the other three milestones were built to keep.

## Solution

Summary is generated **locally by default**, by a Core-supervised llama.cpp sidecar that ships in the box (ADR-0031), so the default-local promise holds on a fresh machine with nothing else installed. Generation enters through one seam (**Backend**) with three implementations: the local sidecar, an OpenAI-compatible cloud client, and a fake that lets every policy test run without a model or a network.

The Knob is the Operator's, and it is explicit: no preselection, Local recommended, and choosing Cloud triggers a hard one-time warning (ADR-0013). Cloud presets carry verified data-handling labels and never gate the choice; the custom base-URL field stays fully open (ADR-0010). API keys live only in the OS credential store (story 41).

Failure recovery has exactly one direction. A failing cloud Backend falls back to local; **local never falls back to cloud**, because that would turn a network blip into an exfiltration. Strict Mode turns the fallback off entirely for Operators who would rather be told than helped (stories 38, 39), and the active Backend is always visible.

**Notes** (ADR-0018) arrive here too, because they are what makes Summary good: what the Operator bothered to write down is the strongest signal of what mattered. They are the one mutable thing in the record, and that refines rather than contradicts ADR-0009.

## User Stories

Numbering follows `docs/prd.md`.

34. As an Operator, I want a post-meeting Summary generated locally by default, so that ending a meeting never silently ships its transcript anywhere.
35. As an Operator, I want the Summary to end with extracted action items, so that follow-ups don't require re-reading.
36. As an Operator, I want enabling cloud Summary to require a hard one-time warning, so that the single biggest possible exfiltration is impossible to enable unknowingly.
37. As an Operator, I want a local/cloud Knob on Summary — the one LLM feature — so that its privacy/quality trade is mine to make.
38. As an Operator, I want cloud-Backend failures to auto-switch cloud→local only — never local→cloud — with a visible active-backend indicator, so that a network blip can't betray a privacy choice.
39. As an Operator, I want Strict Mode ("never auto-switch; tell me on failure"), so that I can trade resilience for absolute predictability.
40. As an Operator, I want curated cloud presets carrying verified data-handling labels (training, retention, ZDR), plus a fully open custom base-URL field, so that I'm informed but never gated.
41. As an Operator, I want API keys stored only in the OS credential store, so that secrets never appear in the database, mirrors, or logs.
42. As an Operator, I want Summary's system prompt exposed as an editable field with reset-to-default, so that I can shape my notes without waiting for a feature.

Carried in from ADR-0018 and the M2 title chain: **Notes** as a first-class mutable entity that steers generation, and the **transcript-derived title suggestion** that sits between calendar and detected-app in the title chain.

## Implementation Decisions

- **The Backend seam**: one trait taking a prompt and returning generated text, with cancellation and streaming-shaped progress. Three implementations — local sidecar, OpenAI-compatible cloud, and a fake. Every Knob, fallback and prompt-armor test drives the fake. This is the fourth time this shape is used (AudioSource, DetectionSource, Diarizer) and the reason is unchanged: the decisions worth testing are policy, and pinning them to a 2 GB model or a network would make them untestable.
- **Local is a sidecar process, not an in-process library** (ADR-0031). The evidence is in the ADR: the one competitor who embedded llama.cpp in-process abandoned it. A process boundary buys crash isolation from the Core — and the Core is the thing that must never die, because it is recording.
- **Sidecar operational constants come from the catalog**, not from invention: JSONL over stdio, lazy model load on `generate`, keep-alive ping, idle self-exit, stdin-EOF orphan protection, per-request timeout, kill-as-cancel (llama.cpp cannot be interrupted), graceful drain then SIGKILL, and **incremental UTF-8 decode so a CJK character split across tokens is not corrupted**. That last one is not a nicety for this product: M1 already paid for Chinese handling once (DECISIONS Q11–Q13).
- **Prompt armor in two layers** (catalog M4): numbered system rules that tell the model to ignore instructions inside the transcript, **plus** escaping of control markers in the transcript text itself. A meeting transcript is untrusted input — anyone who spoke in it can attempt an injection, and "summarize this meeting" is a request to process attacker-controlled text. Canary fixtures for both layers.
- **Map-reduce for long meetings** (catalog M4): single-pass below a token threshold; chunked with overlap above it; per-chunk failure tolerated; cancellation between chunks. A ninety-minute meeting must not simply fail.
- **The output contract** (catalog M4): the title is the first `#` heading of the generated markdown, and action items are a table citing the transcript segment and timestamp for each. A Summary that cannot be traced back to what was said is a Summary nobody can check.
- **The Knob's asymmetry is structural, not a setting**: the fallback path exists only in the cloud→local direction. There is no code path from local to cloud, so no bug, timeout, or retry can produce one. Strict Mode disables even the permitted direction.
- **Keys never touch the record** (story 41). The credential store on each platform, and the guarantee test that already checks nothing key-shaped reaches the database, Mirrors or logs becomes load-bearing rather than theoretical, since M4 is the first milestone that holds a key at all.
- **Sanctioned Traffic gains its third entry, and only its third** (ADR-0034). With the Knob on Local, the zero-network test must still pass — including while a Summary is generating.
- **Summary is not an Anchor** (ADR-0002). Transcription and Diarization are permanently local; Summary is the one feature where the Operator may choose otherwise. Nothing in this milestone may make Summary a dependency of the record.

## Testing Decisions

- **Philosophy (unchanged)**: external behavior only. The observable outputs here are the Summary and Notes that exist afterward, which Backend was used, what reached the network, and what the Mirrors say.
- **The seam is the harness**: `FakeBackend` replaces both the sidecar and the network in every policy test, and this milestone builds the **prompt-injection canaries** the catalog names — a transcript that tries to override the system prompt, and one carrying raw control markers.
- **The tests that matter are the ones that would leak**: local never falls back to cloud, under any failure; Strict Mode never switches at all; a Summary generated on Local produces zero network traffic; and no key reaches the database, a Mirror, or a log.
- **The fallback tests must drive real failure shapes**, not a boolean: a refused connection, a 401, a timeout mid-stream, and a malformed response. A fallback that only works for the failure the author imagined is the one that will not fire.
- **Both platforms, every ticket** (ADR-0025 as amended). The sidecar is a second binary to build, sign, and locate on two platforms, and the credential store is two different APIs — this milestone's Windows column is as real as M2's was.
- **M4 owes a number too**: the close-out reports Summary quality on real meeting audio the way M1 owed WER, M2 owed detection rates and M3 owed DER. What that number *is* — action-item precision and recall against a hand-labelled meeting — is the close-out's to define and defend.

## Out of Scope

- M5: onboarding's final copy (the Backend picker's first-run placement is specified by ADR-0013 but the linear onboarding flow is M5), the floating mini-indicator, distribution and signing.
- Post-v1: Apple Foundation Models as an opportunistic local tier (ADR-0031 names it explicitly as post-v1), streaming Summary into the UI as it generates, multi-model comparison.
- Ratified non-goals reaffirmed: no telemetry, no cloud Transcription or Diarization in any form, no local→cloud fallback, no gating on provider labels, and no Summary content in the Anchors' path.

## Further Notes

- **M3's lesson, which is the same lesson every milestone here has taught in a new costume.** M3 owed a DER, and producing one found three defects that every unit test had passed — including code that had run correctly on fixtures for a week. The M4 form is a Summary that reads beautifully on a short clean transcript and falls apart on ninety minutes of real meeting: chunk boundaries that drop the middle, action items attributed to the wrong speaker, a CJK character corrupted at a token boundary. **The close-out has to run it on a real meeting, and a real meeting is long.**
- The catalog's M4 section carries an operational constant for nearly every decision above — timeouts, thresholds, chunk sizes, priority classes. Consult it before inventing a number; where one is invented anyway, say so where it is written.
- This is the first milestone where a bug can *leak meeting content*. The other three could lose a recording, mislabel a speaker, or miss a meeting — all recoverable. Sending a transcript to a provider the Operator did not choose is not recoverable, and the asymmetry of the fallback path is the main structural defence.
