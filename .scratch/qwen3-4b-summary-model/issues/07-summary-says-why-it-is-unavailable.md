# 07: Summary says why it is unavailable

**What to build:** When Summary cannot run, the Operator learns which of four things is
true — the model is still downloading and how far along, it is absent, it would not start,
or it would not fit this machine — because the action differs completely between them.

Today all four collapse into "the local Summary model is not available." With Local
preselected and a 2.5 GB fetch in flight, the most likely case is the one that sentence
describes worst: a model that is arriving, with a gigabyte of it already on disk.

**Blocked by:** 05, 06 (the progress figure comes from there).

**Status:** done

- [x] Reports megabytes so far against the total. Writing the test found the distinction the Downloader already drew and I had not: a short file at the *final* name is corrupt, while an in-flight download lives at a partial path — one is arriving, the other is wrong, and conflating them would have made the common case report as damage
- [x] Four cases in total, including corrupt. Would-not-fit is inferred from the model being present and verified yet refusing to load, which on a 2.5 GB model is the machine rather than the file
- [x] Driven through `summarize_meeting` with nothing staged, and with a partial file staged; no model is loaded by either
- [x] The reasons are Core-side, where the existing Backend errors already live, so no new catalog keys were needed
- [x] The local gate is green
