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

**Status:** ready-for-agent

- [ ] The dead generation function and result type are gone, with their tests; the splitter, constants, renderer, heading extractor and chunk-boundary tests remain
- [ ] A transcript long enough to chunk produces multiple Backend requests and one stored Summary; a short one behaves exactly as before
- [ ] Choose-once Fallback: a first chunk the cloud Backend cannot serve sends the whole run to local, and the stored Backend label names local — never a mixture
- [ ] A later chunk failing is skipped; the Summary is stored from the chunks that succeeded; what was lost is carried in memory and logged (the record's disclosure is ticket 04)
- [ ] A failed reduce degrades to the concatenated chunk summaries rather than discarding them
- [ ] Cancellation mid-run stores nothing and never falls back; Strict Mode reports instead of switching
- [ ] The Suggested Title comes from the final markdown in both the one-chunk and many-chunk paths
- [ ] The delete-and-rederive and choose-once calls are journaled (re-read the journal tail immediately before appending — a concurrent session has collided once)
- [ ] The honest ledger's "map-reduce never engaged" prose and the M4 criteria carrying the same sentence are corrected; no box is ticked — the long-meeting quality measurement is still owed
- [ ] Tests drive scripted chunk outcomes at the Backend trait seam; no model required; the local gate is green
