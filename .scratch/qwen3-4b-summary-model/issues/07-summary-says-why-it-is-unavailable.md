# 07: Summary says why it is unavailable

**What to build:** When Summary cannot run, the Operator learns which of four things is
true — the model is still downloading and how far along, it is absent, it would not start,
or it would not fit this machine — because the action differs completely between them.

Today all four collapse into "the local Summary model is not available." With Local
preselected and a 2.5 GB fetch in flight, the most likely case is the one that sentence
describes worst: a model that is arriving, with a gigabyte of it already on disk.

**Blocked by:** 05, 06 (the progress figure comes from there).

**Status:** ready-for-agent

- [ ] Still downloading is its own case and names the progress
- [ ] Absent, would-not-start, and would-not-fit are distinguished from each other
- [ ] Each case is driven through the existing Backend seam with no model loaded
- [ ] The strings exist in both locales
- [ ] The local gate is green
