# Summary chunking and the Suggested Title

Status: ready-for-agent

Synthesized from a grilled design session (2026-09-01); every decision below was put
to the Operator and accepted. Glossary terms per `CONTEXT.md` — **Title Chain** and
**Suggested Title** were added to it as part of this design. Governing ADRs: 0030 as
amended by 0036 (the Title Chain), 0013 (the Knob), 0028 (additive-only protocol),
0020 (Nothing Ambient), 0026 (the Core is the only writer), 0009 (immutable record).

## Problem Statement

Two things the product already promises do not happen.

A long meeting — the product's core case — is summarized in a single request. The
chunked map-reduce path exists, is tested, and is called by nothing: an Operator who
records ninety minutes hands their Backend a transcript that overflows it, and the
code that was written to prevent exactly that has been dead since M4 shipped around it.

And the Title Chain's third slot is unwired. ADR-0030 ratifies *manual > calendar >
Suggested Title > detected-app placeholder*, but no Summary has ever named a Meeting:
an untitled recording stays "Zoom, Untitled" forever even after a Summary whose first
heading names it perfectly well.

## Solution

Delete the dead module rather than adopt it, and re-derive chunking inside the
summarize path itself, designed around the Knob from the start: the first chunk's
outcome chooses the Backend for the whole run, later chunk failures are tolerated
rather than switched away from, and what a Summary lost is recorded beside it.

When the finished Summary carries a first-level heading and the Meeting has no
committed name, that heading is written once as the Meeting's **Suggested Title** —
after which it is a name like any other: later Summaries never touch it, and only a
person (or clearing the name, which re-opens the slot) can change it.

## User Stories

1. As an Operator, I want a ninety-minute Meeting summarized in overlapping chunks, so that the Summary covers the whole meeting instead of whatever fit in one request.
2. As an Operator, I want a short Meeting summarized exactly as it is today, so that the common case pays nothing for the long one.
3. As an Operator, I want an untitled Meeting named by its first Summary's heading, so that my History reads as a list of meetings rather than a list of apps and dates.
4. As an Operator, I want a title I typed to survive every Summary regeneration, so that the machine never overwrites my word with its own.
5. As an Operator, I want a calendar-born title to block the Suggested Title, so that the name my calendar committed to outranks a guess from the transcript.
6. As an Operator, I want regenerating a Summary to leave an already-suggested title alone, so that my Meeting's name is stable rather than churning with every regenerate.
7. As an Operator, I want clearing a Meeting's name to re-open the Suggested Title slot, so that "start over" includes letting the next Summary name it.
8. As an Operator, I want a cleared name and a never-set name to behave identically, so that two states my screen renders the same way cannot behave differently behind it.
9. As an Operator, I want a headingless Summary to propose nothing, so that a degenerate model output never becomes my Meeting's name.
10. As an Operator, I want the whole Summary produced by one Backend, so that one record never stitches two models' voices under a label naming only one of them.
11. As an Operator, I want the cloud→local Fallback decided at the start of a run, so that a mid-meeting hiccup skips a chunk instead of silently changing who read my transcript.
12. As an Operator using Strict Mode, I want a failing Backend reported rather than replaced, so that the Knob means what ADR-0013 says it means.
13. As an Operator, I want cancelling a Summary to store nothing — no partial Summary, no title — so that cancel means cancel.
14. As an Operator, I want a Summary built from five of six chunks to say beside it what was lost, so that a partial record never wears the face of a complete one.
15. As an Operator, I want the gap note kept apart from the Summary text and from my Notes, so that machine bookkeeping is never confused with either the model's words or mine.
16. As an Operator, I want a newly suggested title reflected in the Mirror's filename, so that the file on disk and the Meeting in the Client agree about the name.
17. As an Operator with two Clients open, I want the second Client to learn of the new name, so that every window shows the same Meeting.
18. As an Operator, I want upgrading the app to rename nothing, so that no migration surprise sweeps through my History and its Mirror files (Nothing Ambient).
19. As an Operator, I want an old Meeting to gain a Suggested Title only when I regenerate its Summary, so that names change only inside an action I just took.
20. As an Operator on the local Backend, I want all of this without any network traffic, so that the Sanctioned Traffic list stays exactly as long as it is.

## Implementation Decisions

- **Delete, don't adopt.** The dead generation module and its result type are removed
  with their tests. The chunk splitter, its size and overlap constants, the transcript
  renderer, and the heading-extraction helper survive; the chunk-boundary tests stay.
  The stale "reserved for this milestone" comment is rewritten to describe what exists.
- **Map-reduce re-derived inline in the summarize path.** Chunks split on line
  boundaries with overlap (existing splitter). One chunk short-circuits to today's
  single-request behaviour. Multiple chunks: per-chunk summaries, then one reduce
  request; a failed reduce falls back to the concatenated chunk summaries rather than
  discarding them.
- **Choose-once Knob.** The first chunk's outcome selects the Backend for the entire
  run — cloud→local Fallback fires only if the first chunk cannot be served
  (unreachable/unavailable). Later chunk failures are skipped under the tolerance
  rule, never switched. Cancellation never falls back. Strict Mode reports instead of
  switching. The stored Backend label names the one Backend that produced everything.
- **Suggested Title semantics.** The heading extractor runs on the final scrubbed
  markdown. The result is written into the Meeting's title only where the title is
  NULL, in the same store write as the Summary itself. Write-once: regeneration never
  refreshes it. No provenance column — a committed name is a committed name.
- **Retitle normalization.** A retitle whose text is empty or whitespace stores NULL,
  making "cleared" and "never named" one state and re-opening the suggestion slot.
- **Honest degradation: `summary_gaps`.** A new nullable column (additive migration)
  records what the Summary lost, following the established audio-notes pattern —
  deliberately not named "notes", which the glossary reserves for Operator writing.
  Written in the same UPDATE as the Summary so the existing Mirror triggers fire; the
  Mirror renders it beside the Summary. Exposed to Clients as an additive optional
  field per ADR-0028.
- **Announcement.** A title fill announces the Meeting as Updated through the existing
  change-notification, exactly as a manual retitle does; the Mirror renames via the
  existing title-update trigger.
- **No backfill.** No migration writes titles. Old Meetings gain a Suggested Title
  only as a consequence of an Operator-triggered regeneration.

## Testing Decisions

Tests assert external behaviour at existing seams — no new seams are introduced:

- **The Backend trait seam** (existing fake Backend): drives chunk-by-chunk outcomes
  deterministically with no model — choose-once Fallback on a dead first chunk,
  skip-and-record on a dead later chunk, reduce-failure degradation, Strict Mode,
  cancellation. Prior art: the existing Knob and generation tests against the fake.
- **The protocol seam** (existing test-Core-over-the-wire idiom): end-to-end — an
  untitled Meeting summarized gains a title, the change announces as Updated, the
  Mirror file is renamed, `summary_gaps` rides the response; a manually titled Meeting
  summarized keeps its name. Prior art: the meeting-lifecycle and live-captions suites.
- What makes a good test here: behaviour-named, asserting what an Operator or Client
  observes (stored title, announcement, Mirror name, gaps note) — never the chunk
  arithmetic or internal call order. Chunk boundary correctness is already covered by
  the surviving splitter tests.
- No test in this spec needs a model. The long-meeting *quality* measurement stays a
  model-gated concern owed to the M4 close-out (see Out of Scope), and any future
  model-gated test follows the set-but-missing-fails idiom (DECISIONS Q43).

## Out of Scope

- Choosing a better default Summary model (M4's open criterion; Q45 evidence stands).
- Measuring Summary quality on a real long meeting — this spec makes that measurement
  *possible* (chunking engages); taking it remains owed to the M4 close-out.
- Refreshing a Suggested Title on regeneration, or any title provenance column.
- Per-chunk Backend mixing, retrying failed chunks, or resumable generation.
- Retroactive backfill of titles for already-summarized Meetings.
- Any change to Sanctioned Traffic, the Knob's direction, or the Briefing.
- Bilingual transcription quality (tracked separately).

## Further Notes

- On landing: journal the delete-and-rederive and choose-once calls in DECISIONS.md
  (re-read the tail immediately before appending — a concurrent session has produced a
  duplicate Q-number once already); correct the "map-reduce never engaged" prose in
  the honest ledger and the M4 criteria carrying the same sentence, boxes untouched
  until the measurement is taken.
- `CONTEXT.md` already carries **Title Chain** and **Suggested Title** with these
  exact semantics; the implementation must match the glossary, not the other way round.
- The chain stays ratified ADR-0030 content; this spec implements it and amends nothing.
