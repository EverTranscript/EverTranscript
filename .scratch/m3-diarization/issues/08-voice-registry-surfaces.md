# 08: The Voice Registry — Client and CLI

**What to build:** The inspection and control surfaces ADR-0008 made **mandatory** in exchange for storing biometrics at all. These are acceptance criteria of this milestone, not follow-up polish: shipping the collection without the controls breaks the bargain the ADR struck.

**Blocked by:** 02, 07.

Status: done

- [x] A **Voice Registry** listing every Speaker the app holds and whether each has a Voiceprint (story 30) — the complete biometric inventory, with nothing held outside it
- [x] Display name, meetings seen in, first appearance, confirmed state, and the model that produced the Voiceprint. **Last appearance is not shown**: it is one more query for a fact that answers no question the other five leave open, and the Registry is an inventory rather than an activity feed. Counts are derived from segments rather than stored, so they cannot drift from what they describe
- [x] **Delete a Voiceprint** from the Registry (story 31), in both surfaces, with the consequence stated **before** the act in each — the CLI prints it above a y/N prompt and the Client expands it inline before the confirm button. A biometric deletion that explains itself afterwards is a surface that has already spent the Operator's trust
- [x] Rename a Speaker from the Registry, with the retroactive consequence stated plainly — this is the act that composes with deletion into de-identification (story 32)
- [x] The CLI carries every one of these surfaces, because the Operator's record stays scriptable (the standing story 16 obligation): list Speakers, show one, rename, delete a Voiceprint, and re-assign a segment
- [x] `evertranscript diarize status` and `diarize cancel` carry it in the CLI. **The Client shows no job indicator yet** and that is deliberate rather than forgotten: nothing produces a real Diarization until ticket 03, so an indicator would be a permanently idle widget. The protocol and the CLI are in place for it to bind to
- [x] Client strings are externalized for EN + zh-CN like every UI string since M1
- [x] The Registry is reachable without a Meeting open, because it is an inventory of the app's holdings and not a property of any one recording
