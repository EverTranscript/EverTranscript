# 02: A Summary names an untitled Meeting

**What to build:** The Title Chain's third slot fires (ADR-0030 as amended): when a
Summary's final markdown opens with a first-level heading and the Meeting has no
committed name, that heading becomes the Meeting's Suggested Title — written once, in
the same store write as the Summary, announced as Updated, the Mirror renamed. On
today's single-request path; chunking is ticket 03. Semantics per the CONTEXT.md
glossary entry, which is normative here.

**Blocked by:** 01 (clearing a name means unnamed — the escape hatch must exist first).

**Status:** ready-for-agent

- [ ] Summarizing an untitled Meeting whose Summary carries a first-level heading names the Meeting with that heading, atomically with the Summary write
- [ ] The fill announces the Meeting as Updated and the Mirror file is renamed
- [ ] A manually named Meeting keeps its name through summarize; a calendar-born name likewise
- [ ] Regenerating a Summary never changes an already-filled name
- [ ] A headingless Summary proposes nothing; the placeholder stands
- [ ] Clearing the name and regenerating fills it again — the slot re-opened by ticket 01's normalization
- [ ] A cancelled generation stores neither Summary nor name
- [ ] The stale "reserved for this milestone" comment is replaced by a description of what now exists
- [ ] Tests at the protocol seam against the fake Backend; no model required; the local gate is green
