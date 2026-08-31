# 09: M3 close-out — the measured DER, the bake-off, the guarantees

**What to build:** The evidence that Diarization works, on both platforms, measured rather than asserted. M1's close-out owed a WER; M2's owed false-negative rates; this one owes a **Diarization Error Rate**.

**Blocked by:** 03, 04, 05, 06, 07, 08.

Status: not started

- [ ] **DER measured against labelled audio, reported as a number**, with the fixture set and the labelling method named so it can be re-run
- [ ] **DER measured on at least one real recorded meeting, not only the fixture library.** M2 ended with six defects of one family: the code believed something the machine did not honor, and every test agreed because the tests shared the belief. The M3 form of that is a pipeline that scores well on curated audio and falls apart on a real meeting. **Fixture audio is evidence about fixtures**
- [ ] **The embedding bake-off is decided on that measurement** (catalog M3: WeSpeaker-family vs ReDimNet), with the losing option's numbers recorded rather than discarded — a bake-off whose loser is not written down is a preference wearing a lab coat
- [ ] Cross-meeting recognition measured, not just demonstrated: over a set of meetings with known returning speakers, how often is a returning Speaker correctly recognized, and how often is a stranger wrongly matched to one. Both numbers, since the second is the one that damages trust
- [ ] Story 29 end-to-end on real data: name a Speaker, and every past appearance and every affected Mirror updates
- [ ] Story 31 end-to-end: delete a Voiceprint, confirm recognition stops and the record is byte-for-byte unchanged
- [ ] Guarantee tests extended rather than restarted: the zero-network test passes **with diarization running**, the permission set is unchanged (diarization needs no grant), and the framework audit still forbids what it forbade
- [ ] The crash suite covers a Meeting killed mid-diarization, recovering with whatever attribution completed and no half-written state
- [ ] **Both platforms green in CI, with the ONNX pipeline actually executing on `windows-latest`** — not merely compiling and linking, which is precisely the condition under which M2's Windows detection was green and had never worked
- [ ] The dogfood proof: a real meeting recorded, diarized, with Speakers named, correct after one Operator correction, and the Mirror readable as a conversation
