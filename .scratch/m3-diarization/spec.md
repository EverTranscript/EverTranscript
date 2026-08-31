# M3 — Diarization: Speakers, Voiceprints, "You", and the Voice Registry

Status: ready-for-agent

Sources of truth: `CONTEXT.md` (glossary — its vocabulary is normative here), `docs/prd.md` (stories 27–33), ADR-0008 (persistent Speakers, as amended), ADR-0009 (immutable record + correction hints, as amended), ADR-0029 (dual-channel, "the mic channel is *where the Operator is*", as amended), ADR-0035 (Voiceprints in History, plaintext, no TTL), ADR-0026 (Core is the only writer), ADR-0028 (protocol additive-only), `docs/implementation-notes-2026-08-27.md` (the absorption catalog — its M3 section has an evidence path and a constant for nearly every decision below; consult it per the reuse rules in `AGENTS.local.md`). Where this spec and an ADR disagree, the ADR wins.

## Problem Statement

M1 made a record worth trusting and M2 made it start by itself. Both produce a Transcript that says *what was said* and cannot say *who said it*. `transcript_segments.speaker_id` has been nullable and null since M1, and the `speakers` table has held a Voiceprint column that nothing writes.

That gap is not cosmetic. The PRD's headline recall story — "what did Alice say last month" — is unreachable without persistent Speakers, and so is the retroactive-naming act that makes one rename organize a whole History (story 29). The Mirrors currently read as undifferentiated walls of text, which is the form in which a meeting record is least useful.

It is also the milestone where this product takes on its heaviest obligation. ADR-0008 stores biometric identifiers for Participants who never consented, as a side effect of recording. The ADR accepts that eyes-open and names the price: the Voice Registry, per-Speaker Voiceprint deletion, and visible match attribution are **mandatory legibility surfaces**, not polish. A milestone that ships clustering without them ships the exposure without the controls.

## Solution

Diarization runs **post-meeting and entirely locally** (story 33 — no cloud form exists at all), on **both channels** (ADR-0029 as amended), producing per-segment Speaker attribution and voice embeddings. Every voice resolves to a persistent Speaker; naming one is retroactive across all of History because attribution is a live reference, never text baked into the Transcript (ADR-0009).

Diarization enters through one seam (**Diarizer**), the way capture entered through AudioSource in M1 and detection through DetectionSource in M2, so the policy that turns embeddings into Speakers is testable without models, meetings, or a GPU.

The Operator gets an automatic persistent Speaker displayed **"You"**, matched on the mic channel by Voiceprint and bootstrapped from the dominant mic voice over the first Meetings. The channel prior is a strong hint, not an axiom: a shared conference room puts other real voices on the mic channel and they cluster as ordinary Speakers.

The human-feedback loop closes in both directions. Naming a Speaker **confirms** its Voiceprint into a higher-trust matching tier. A mis-attribution correction is an **appended hint** that wins display and the Mirrors while the machine's original conclusion is preserved beneath it — so the record stays auditable and re-diarization stays possible.

## User Stories

Numbering follows `docs/prd.md`.

27. As an Operator, I want post-meeting Diarization to attribute the Transcript to Speakers, so that I know who said what.
28. As an Operator, I want every voice to resolve to a persistent Speaker across Meetings, so that "what did Alice say last month" works without ceremony.
29. As an Operator, I want naming a Speaker to retroactively label all their past appearances, so that one act organizes my whole History.
29b. As an Operator, I want to re-assign a mis-attributed segment to the right Speaker — my correction layering above the machine's attribution, never rewriting it — so that recognition errors are mine to fix and the machine learns from the fix (ADR-0009 as amended).
30. As an Operator, I want a Voice Registry listing every Speaker and Voiceprint the app holds, so that the biometric inventory is fully inspectable.
31. As an Operator, I want to delete any Speaker's Voiceprint, so that the app stops recognizing them without touching the record.
32. As an Operator honoring a Participant's deletion request, I want Voiceprint deletion plus Speaker rename to compose into de-identification, so that I can honor the request to the degree I choose.
33. As an Operator, I want Diarization to run only locally with no cloud form at all, so that voices are never a network question.

Carried from M2 rather than new: attendee names stored by calendar arming become **Speaker-naming suggestions** here (M2 stored them; M3 suggests from them — never applies them, since turning an invitation into an attribution is inventing who spoke).

## Implementation Decisions

- **The Diarizer seam**: one trait taking a finished Meeting's two channels and producing turns — `(channel, start, end, cluster)` — plus per-cluster embeddings, with a live implementation over ONNX and a fixture implementation replaying scripted turn timelines. Every attribution, persistence, and naming test drives the fixture; the ONNX pipeline is tested for what only it can be, that it segments and embeds real audio correctly. **M1 and M2 both paid for the same lesson and it applies again**: fixtures deliver whole, tidy artifacts and the real thing arrives fragmented and ugly, so the fixture must be able to produce short turns, overlapped speech, and single-speaker meetings, not only clean three-way conversations.
- **The pipeline is the pyannote-family shape** (ADR-0029, catalog M3): powerset segmentation ONNX + speaker-embedding ONNX via `ort`, 80-mel fbank features computed in **pure Rust** (the reference does it in pure JS, so no C dependency is warranted), agglomerative clustering. Cross-platform by construction, which is what makes the ADR-0025 parity gate reachable here at all.
- **Two embeddings, two jobs** (catalog M3): a cheap clustering embedding used within a Meeting, and a durable identity embedding used as the stored Voiceprint. They are different models with different cost and stability requirements, and collapsing them is the mistake that makes either clustering slow or recognition brittle.
- **Cross-meeting persistence is a bias, not a special case** (catalog M3): seed each Meeting's clusterer with prior Voiceprints as frozen speakers at negative timestamps. The clusterer then has one code path, and recognition falls out of clustering rather than sitting beside it as a second matching system that can disagree with it.
- **Reconciliation is interval overlap on one clock** (ADR-0009's join, catalog M3): ASR words and diarization turns share the absolute capture clock ADR-0029 already mandates; each word takes the Speaker whose turn contains its **midpoint**. Attribution arrives after the Transcript is already published, so it re-maps published segments — and the count of boundary flips is a quality metric worth keeping, not a wart to hide.
- **Matching is conservative and says so** (catalog M3, reference numbers): a match requires cosine above a floor **and** a margin over the second-best candidate **and** mutual-best agreement in both directions. A confident wrong attribution is worse than an unnamed Speaker, because the Operator has to notice it before they can correct it.
- **Naming is confirmation** (ADR-0008 as amended): naming promotes the Voiceprint to an Operator-confirmed tier that wins ties thereafter. Unconfirmed Voiceprints still match, conservatively. Unnamed Speakers are numbered pseudonyms — "Speaker 1", "Speaker 2".
- **Corrections append, never rewrite** (ADR-0009 as amended): a re-assignment is a hint row. Display and Mirrors follow the hint; the machine's attribution stays underneath; the correction feeds the right Speaker's exemplars as positive evidence and the wrong one's as negative.
- **Speaker records are permanent; Voiceprints are deletable** (ADR-0009). There is no anonymize mechanism because rename already is one, and no utterance excision because a record that self-edits is the opposite of a legible guarantee.
- **Batch compute is reject-don't-queue** (catalog M3): an overlapping diarization job is refused rather than queued, FFI is wrapped in `catch_unwind`, work proceeds in bounded windows, and the job is cancellable. Live transcription must not be starved by post-meeting diarization — M1 already paid for the version of this mistake where transcription starved capture (DECISIONS Q7).
- **Nothing new leaves the machine.** Diarization adds no network path in any form. The zero-network guarantee test must hold with diarization running, and the model downloads join the existing checksummed first-run set (ADR-0034's Sanctioned Traffic, unchanged).

## Testing Decisions

- **Philosophy (unchanged)**: external behavior only. The observable outputs here are the Speakers that exist afterward, the attribution on segments, what the Mirrors say, and the protocol events Clients saw — not the internals of a clusterer.
- **The seam is the harness**: `FixtureDiarizer` replaces models in every policy test, and this milestone builds the **turn-timeline fixtures** the way M2 built detection-event fixtures: a clean two-speaker conversation, a solo meeting, a shared room (two voices on the mic channel), a returning speaker from a previous Meeting, overlapped speech, and a meeting with one very short turn.
- **The tests that matter are the ones that cost the Operator trust**: a returning Speaker must be recognized across Meetings; naming must propagate retroactively to every past appearance and to the Mirrors; a correction must win display without erasing the machine's conclusion; deleting a Voiceprint must stop recognition and change nothing in the record.
- **The pipeline gets its own empirical matrix, and it is measured**: **Diarization Error Rate against labelled fixture audio** is this milestone's owed number, the way M1 owed WER and M2 owed false-negative rates. The WeSpeaker-family vs ReDimNet embedding bake-off (catalog M3) is decided on that measurement, not on reputation.
- **Guarantee tests extend, not restart**: the permission set gains nothing (diarization needs no grant); the zero-network test must pass with diarization active; the crash suite must cover a Meeting whose diarization was interrupted, since a half-attributed Transcript that never completes is a new way to corrupt the record.
- **Both platforms, every ticket** (ADR-0025 as amended). M2 ended by discovering that Windows detection had never worked at all while CI was green — a whole platform failing silently because the code compiled and nothing ran it. `ort` and ONNX on Windows is exactly that shape of risk again.

## Out of Scope

- M4: Summary, Notes, the provider Knob, the transcript-derived title suggestion.
- M5: the full Briefing (M1's minimal acknowledgment still stands in), linear onboarding, the floating mini-indicator, distribution.
- Explicitly not now: live/streaming diarization (this is post-meeting by ADR decision), speaker *identification* against any external corpus, enrollment ceremonies (Speakers are born from meetings, not from a setup wizard), and the provisional-unratified controls ADR-0008 names — a clustering master switch and purge-all-Voiceprints — which stay unratified until an ADR says otherwise.
- Ratified non-goals reaffirmed: no cloud diarization in any form, no utterance excision, no dedicated anonymize mechanism, no Voiceprint TTL, no telemetry.

## Further Notes

- **M2's closing lesson is the one to carry in.** That milestone shipped six defects of one family: the code believed an identifier or an API contract that the machine did not honor, and every test agreed with the code because the tests were written from the same belief. Five needed real hardware to expose; the sixth needed only reading the documentation for a call that compiled cleanly. The M3 analogue is a diarization pipeline that scores well on curated fixture audio and falls apart on a real meeting — different tensor layout, different sample rate assumption, a model whose output is a probability where the code reads a logit. **Fixture audio is not evidence about meetings; it is evidence about fixtures.** The close-out ticket owes a DER on real recorded audio, not only on the library.
- The absorption catalog's M3 section carries a constant for nearly every threshold above — merge threshold, window and hop, span selection, match floors and margins. Consult the referenced source before inventing a number; where a number is invented anyway, say so where it is written.
- ADR-0008's legibility surfaces are acceptance criteria of this milestone, not follow-up work. The ADR made a bargain — biometric collection in exchange for inspectability — and shipping only the collecting half breaks it.
- The `speakers` table, its Voiceprint columns, and `transcript_segments.speaker_id` have existed since M1 and have never been written to. Treat the existing shape as a proposal from a session that had not yet built this, not as settled schema.
