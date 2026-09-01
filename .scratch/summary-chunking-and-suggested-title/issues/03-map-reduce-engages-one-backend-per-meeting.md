# 03: Map-reduce engages, one Backend per Meeting

**What to build:** A long Meeting is summarized in overlapping chunks in production —
the capability that has existed, tested and unreachable, since M4. The dead generation
module and its result type are deleted with their tests; the splitter, its size and
overlap constants, the transcript renderer, the heading extractor and the boundary
tests survive. Chunking is re-derived inline around the Knob: **choose once** — the
first chunk's outcome selects the Backend for the whole run, and everything after
follows tolerance, not switching. Title fill (ticket 02) is preserved on the final
markdown for one chunk and many.

**Blocked by:** 02 (both rewrite the same summarize body; the fill must survive the rewrite).

**Status:** done

- [x] The dead function and result type are gone with their six tests (101 lines); the splitter, constants, renderer, heading extractor and the seven rendering/chunking tests remain and pass
- [x] A 400-line transcript produces several chunk requests plus a reduce — counted through the fake's recorded prompts, which is the only way to tell chunking from one long request; a 3-line one still makes a single call
- [x] Choose-once Fallback: a cloud Backend that fails the first chunk sends the whole run to local, the stored label says Local, and local's own prompt log shows it served every chunk rather than just the first
- [x] A later chunk failing is skipped and the Summary still stores; the loss is counted on the run and logged with `failed` and `of` counts. The record's disclosure is ticket 04
- [x] A failed reduce keeps the parts — asserted by finding an early chunk's text in the stored Summary
- [x] Every chunk failing is an error and stores nothing. Cancellation and Strict Mode are unchanged: both are decided inside `knob::run`, which still owns them, and the re-derived loop checks the cancel flag between chunks and before the reduce
- [x] The Suggested Title comes from the reduced markdown, not a chunk's — tested with three different headings where only the reduce's may win
- [x] Journaled as Q56, after re-reading the tail — the concurrent session had reached Q55 while this ticket was in progress
- [x] Both corrected, and both now say the sharper thing: it was not that the meeting was too short, it was that no meeting of any length would have chunked. No box ticked — the measurement is still owed
- [x] Seven tests driving scripted chunk outcomes through the injected Backend factory; no model; the local gate is green
