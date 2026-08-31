# 05: "You" — the Operator's Speaker, and the channel prior that is only a prior

**What to build:** The Operator's automatic persistent Speaker, and channel-aware attribution that survives a conference room.

**Blocked by:** 04.

Status: not started

- [ ] An automatically created persistent Speaker for the Operator, displayed **"You"**, matched on the mic channel by Voiceprint (ADR-0029 as amended)
- [ ] Bootstrapped from the **dominant mic voice over the first Meetings** — not from an enrollment ceremony, because Speakers in this product are born from meetings
- [ ] **The channel prior is a hint, not an axiom.** ADR-0029 was amended precisely to soften "the mic channel *is* the Operator" to "is *where the Operator is*". Other mic-channel voices — a shared room, a colleague leaning in — cluster as ordinary Speakers. A design that cannot represent a second voice on the mic channel falsifies every conference-room recording, silently
- [ ] The shared-room fixture from 01 is the test that matters here, and it must produce two Speakers on the mic channel with the Operator correctly identified as one of them
- [ ] The solo case still gets the benefit: with one mic voice, the channel prior is strong evidence and attribution should be correspondingly confident
- [ ] "You" is a display name, not a magic record: it is a Speaker like any other in the Registry, inspectable and with a deletable Voiceprint (stories 30, 31). The Operator can rename it
- [ ] AEC is already applied from M1, so far-end voices should not be on the mic channel at all — but the test must include echo-contaminated fixture audio anyway (ADR-0029's required case), because a broken AEC presenting as a phantom room-mate is exactly the failure this would otherwise hide
