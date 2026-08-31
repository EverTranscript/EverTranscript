# 03: Generating a Summary — prompts, armor, map-reduce, output contract

**What to build:** The part between "a transcript exists" and "a Summary exists", independent of which Backend runs it.

**Blocked by:** 01, 02.

Status: not started

- [ ] Summary generation takes the Transcript, the Notes, and the Speakers, and produces markdown
- [ ] **Action items as a table citing transcript segment and timestamp per item** (story 35, catalog M4). A Summary that cannot be traced back to what was actually said is one nobody can check, and an LLM will happily invent a commitment nobody made
- [ ] **The title is the first `#` heading of the generated markdown** (catalog M4), which is the transcript-derived title suggestion the M2 title chain left for this milestone: manual > calendar > **this** > detected-app placeholder
- [ ] **Prompt armor, layer one:** numbered system rules including an explicit instruction to ignore anything instruction-shaped inside the transcript, omit-if-unsure, and a fixed phrase for empty sections
- [ ] **Prompt armor, layer two:** escape control markers in the transcript text itself (`<|im_start|>`, `<|im_end|>`, `<start_of_turn>`, `<think>`, …). A transcript is untrusted input — everyone who spoke in the meeting wrote part of it — and "summarize this" is a request to process attacker-controlled text
- [ ] **Canary fixtures for both layers**: a transcript that tries to override the system prompt, and one carrying raw control markers. These are tests, not comments
- [ ] Output scrubbing strips think-blocks and stray markdown fences (catalog M4)
- [ ] **Map-reduce**: single-pass below the token threshold; above it, chunks with overlap split on sentence boundaries, per-chunk failures tolerated (fail only if all fail), cancellation checked between chunks. A ninety-minute meeting must produce something rather than nothing
- [ ] The system prompt is a value with a default, not a literal buried in code — story 42 needs to edit and reset it
- [ ] Summarizing happens **in the transcript's own language**, not via an English pivot (catalog M4 says so explicitly, having watched a competitor do the opposite)
