# 08: The Voice Registry — Client and CLI

**What to build:** The inspection and control surfaces ADR-0008 made **mandatory** in exchange for storing biometrics at all. These are acceptance criteria of this milestone, not follow-up polish: shipping the collection without the controls breaks the bargain the ADR struck.

**Blocked by:** 02, 07.

Status: not started

- [ ] A **Voice Registry** listing every Speaker the app holds and whether each has a Voiceprint (story 30) — the complete biometric inventory, with nothing held outside it
- [ ] Per-Speaker facts a person can act on: display name, meetings seen in, first and last appearance, whether the Voiceprint is Operator-confirmed, and which model produced it
- [ ] **Delete a Voiceprint** from the Registry (story 31): recognition stops, the record is untouched, and the surface says exactly that before it happens rather than after
- [ ] Rename a Speaker from the Registry, with the retroactive consequence stated plainly — this is the act that composes with deletion into de-identification (story 32)
- [ ] The CLI carries every one of these surfaces, because the Operator's record stays scriptable (the standing story 16 obligation): list Speakers, show one, rename, delete a Voiceprint, and re-assign a segment
- [ ] Diarization job state is visible and cancellable from both surfaces — a post-meeting job the Operator cannot see or stop is an unaccountable use of their machine
- [ ] Client strings are externalized for EN + zh-CN like every UI string since M1
- [ ] The Registry is reachable without a Meeting open, because it is an inventory of the app's holdings and not a property of any one recording
