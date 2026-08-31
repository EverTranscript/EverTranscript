# 03: Generating a Summary — prompts, armor, map-reduce, output contract

**What to build:** The part between "a transcript exists" and "a Summary exists", independent of which Backend runs it.

**Blocked by:** 01, 02.

Status: done, except the language check named below

- [x] Summary generation takes the Transcript, the Notes, and the Speakers, and produces markdown
- [x] **Action items as a table citing transcript segment and timestamp per item** (story 35, catalog M4). A Summary that cannot be traced back to what was actually said is one nobody can check, and an LLM will happily invent a commitment nobody made
- [x] **The title is the first `#` heading of the generated markdown** (catalog M4), which is the transcript-derived title suggestion the M2 title chain left for this milestone: manual > calendar > **this** > detected-app placeholder
- [x] **Prompt armor, layer one:** numbered system rules including an explicit instruction to ignore anything instruction-shaped inside the transcript, omit-if-unsure, and a fixed phrase for empty sections
- [x] **Prompt armor, layer two:** escape control markers in the transcript text itself (`<|im_start|>`, `<|im_end|>`, `<start_of_turn>`, `<think>`, …). A transcript is untrusted input — everyone who spoke in the meeting wrote part of it — and "summarize this" is a request to process attacker-controlled text
- [x] **Canary fixtures for both layers**: a transcript that tries to override the system prompt, and one carrying raw control markers. These are tests, not comments
- [x] Output scrubbing strips think-blocks and stray markdown fences (catalog M4)
- [x] **Deviation, and it is a narrowing rather than a shortcut:** chunks split on **line** boundaries, not sentence boundaries. A rendered transcript is already one utterance per line, so a line boundary *is* a sentence boundary here — and it additionally never separates a speaker attribution from the words it labels, which a mid-line split could do, handing a chunk a quotation with no idea who said it. There is a test that every line of the meeting appears in some chunk, because chunking that dropped the middle would be invisible in the output. Original criterion: chunks with overlap split on sentence boundaries, per-chunk failures tolerated (fail only if all fail), cancellation checked between chunks. A ninety-minute meeting must produce something rather than nothing
- [x] The system prompt is a value with a default, not a literal buried in code — story 42 needs to edit and reset it
- [ ] **The instruction is in the prompt (rule 2) and has never been checked against a model.** No local Backend exists yet to check it with, and the fake cannot be persuaded by a prompt — which is exactly what makes it insufficient here. The close-out's canaries-against-a-real-Backend criterion covers this one too, and it stays open until then rather than being ticked on the strength of having written the sentence
