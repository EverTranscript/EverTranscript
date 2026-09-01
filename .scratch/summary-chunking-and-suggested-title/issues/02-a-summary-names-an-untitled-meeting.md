# 02: A Summary names an untitled Meeting

**What to build:** The Title Chain's third slot fires (ADR-0030 as amended): when a
Summary's final markdown opens with a first-level heading and the Meeting has no
committed name, that heading becomes the Meeting's Suggested Title — written once, in
the same store write as the Summary, announced as Updated, the Mirror renamed. On
today's single-request path; chunking is ticket 03. Semantics per the CONTEXT.md
glossary entry, which is normative here.

**Blocked by:** 01 (clearing a name means unnamed — the escape hatch must exist first).

**Status:** done

- [x] Summarizing an untitled Meeting whose Summary carries a first-level heading names the Meeting with that heading. **Atomic**: both UPDATEs run in one explicit transaction inside the same store write, so a crash cannot leave a Meeting named by a Summary it does not have
- [x] The Mirror is renamed — asserted on the filename the Meeting reports. The summarize path now rebuilds pending Mirrors before answering, as retitle already did; the existing `AFTER UPDATE OF title` trigger marks it dirty
- [x] A manually named Meeting keeps its name. Structural rather than checked: the fill is `WHERE title IS NULL`, and slots one and two of the chain both live in that column, so a calendar-born name is covered by the same clause
- [x] Regenerating never changes an already-filled name — same clause, tested with a second Backend returning a different heading
- [x] A headingless Summary proposes nothing, tested with the shape Q45 measured the shipped 0.5B producing
- [x] Clearing the name and regenerating fills it again, which is ticket 01's normalization seen from this side
- [x] A Summary that was never stored names nothing — driven with a failing Backend
- [x] The stale "reserved for this milestone" comment now describes what exists
- [x] Seven behaviour-named tests driving record→summarize→read with a scripted Backend; four were red before the change; no model required; the local gate is green
