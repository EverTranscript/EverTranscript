# 09: M4 close-out — the number, the leak tests, both platforms

**What to build:** The evidence that Summary works and that the Knob holds. M1 owed a WER, M2 owed detection rates, M3 owed a DER. This one owes a Summary-quality number **and** a set of tests that would catch a leak.

**Blocked by:** 03, 04, 05, 06, 07, 08.

Status: not started

- [ ] **A quality number on a real meeting, defined and defended.** Action-item precision and recall against a hand-labelled real recording is the proposal; whatever is chosen, the labelling method is named so it can be re-run, and the number is reported even if it is bad
- [ ] **Measured on a long meeting, not a clean short one.** The M4 failure mode is a Summary that reads beautifully on five minutes and falls apart on ninety: chunk boundaries dropping the middle, action items attributed to the wrong speaker, a CJK character corrupted at a token boundary. Every prior milestone's close-out found defects that its unit tests had passed, and every one of them was found by running the real thing
- [ ] **The leak tests, and they are the point of this milestone**: local never falls back to cloud under any of the four failure shapes; Strict Mode never switches; a Summary generated on Local produces zero network traffic; no key reaches the database, a Mirror, or a log
- [ ] Prompt-injection canaries pass against a **real** Backend, not only the fake — the fake cannot be persuaded by a prompt, which is exactly what makes it insufficient here
- [ ] Guarantee tests extended rather than restarted: Sanctioned Traffic still enumerates to three entries, and the binary still contains no analytics SDK
- [ ] The crash suite covers a Meeting whose Summary was interrupted, and a sidecar that died mid-generation
- [ ] **Both platforms green in CI, with the sidecar actually running on `windows-latest`** — not merely built. M3's CI change found two Windows-only defects before it reached the inference it was added to prove; a second binary is more surface, not less
- [ ] The dogfood proof: a real meeting recorded, transcribed, diarized, Notes written, and a Summary generated locally whose action items are correct — end to end, on one recording, with nothing sent anywhere
