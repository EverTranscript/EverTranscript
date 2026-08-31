# 05: "You" — the Operator's Speaker, and the channel prior that is only a prior

**What to build:** The Operator's automatic persistent Speaker, and channel-aware attribution that survives a conference room.

**Blocked by:** 04.

Status: done

- [x] An automatically created persistent Speaker for the Operator, displayed **"You"**, matched on the mic channel by Voiceprint (ADR-0029 as amended)
- [x] Bootstrapped from the **dominant mic voice over the first Meetings** — not from an enrollment ceremony, because Speakers in this product are born from meetings
- [x] **The channel prior is a hint, not an axiom.** ADR-0029 was amended precisely to soften "the mic channel *is* the Operator" to "is *where the Operator is*". Other mic-channel voices — a shared room, a colleague leaning in — cluster as ordinary Speakers. A design that cannot represent a second voice on the mic channel falsifies every conference-room recording, silently
- [x] The shared-room fixture is the test that matters, and it produces a **stronger** result than the criterion asked for. With no Voiceprint the machine has no way to know which of two balanced mic voices owns the laptop, so it identifies **neither** — an unnamed Speaker is a visible gap and a wrongly-named one is invisible. Given the Operator's Voiceprint it finds them in the same room even though the colleague does most of the talking, which is the case loudness alone gets exactly backwards
- [x] The solo case still gets the benefit: with one mic voice, the channel prior is strong evidence and attribution should be correspondingly confident
- [x] "You" is a display name, not a magic record: it is a Speaker like any other in the Registry, inspectable and with a deletable Voiceprint (stories 30, 31). The Operator can rename it
- [x] Echo contamination is tested, and the assertion is that it stays **visible**: a phantom second mic voice blocks the bootstrap rather than being averaged into "You". A broken AEC then shows up as an unidentified Operator — a thing somebody notices — instead of quietly poisoning the Voiceprint that everything else is matched against
