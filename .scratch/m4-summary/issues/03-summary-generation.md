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
- [x] **Checked against a real model, 2026-09-05.** A Chinese meeting was recorded through the real capture path and summarised on the local Qwen3-4B Backend: the heading (`# 千尼推迟讨论会议`), the discussion and the action item all came back in Chinese, with no translation into English. The criterion said it stays open until a real Backend exists to check it with; one does, so it was checked rather than argued.

  **What that surfaced, and did not fix:** the summary's *structural* labels stay English — the section title "Action items" and the columns "Who | What | When | Said at" — because rule 5 names them literally while rule 2 says do not translate. Two prompt rewrites were tried and measured on the same meeting. Describing the columns in prose instead of naming them made it worse: lowercase English headers and no section title at all. Naming them and instructing the model to translate them changed nothing. A 4B model will not reliably localise labels the prompt spells out, so this is a model limit rather than a wording bug, and the original rule reads better than either attempt. Left as it was, recorded here rather than rediscovered: a Chinese Operator's Summary is Chinese prose under an English table header. Nothing parses those literals, so localising them is safe whenever a model can be relied on to do it
