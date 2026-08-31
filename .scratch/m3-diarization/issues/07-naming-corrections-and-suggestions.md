# 07: Naming as confirmation, corrections as hints, attendees as suggestions

**What to build:** The human-feedback loop — the two ADR amendments this milestone exists to honor, plus the M2 debt that attendee names become suggestions here.

**Blocked by:** 02, 04, 06.

Status: not started

- [ ] Naming a Speaker **retroactively labels every past appearance** (story 29), because attribution is a live reference and not text baked into the Transcript (ADR-0009). One rename, whole History — including every affected Mirror rebuilt
- [ ] **Naming is confirmation** (ADR-0008 as amended): it promotes that Speaker's Voiceprint to an Operator-confirmed tier that wins ties in future matching
- [ ] **A correction is an appended hint, never a rewrite** (ADR-0009 as amended, story 29b): re-assigning a segment writes a hint that wins the display join and the Mirrors, while the machine's attribution is preserved beneath it and remains visible to anyone who asks
- [ ] A correction feeds exemplars in **both** directions: positive evidence for the Speaker it was re-assigned to, negative evidence against the one it was taken from (catalog M3, human-feedback policy)
- [ ] **Visible match attribution** (ADR-0008's mandatory legibility): a Transcript can say why a segment was attributed as it was — matched Voiceprint, channel prior, clustered-only, or Operator correction. An unexplained biometric guess is the thing the ADR bargained against
- [ ] **Attendee names become suggestions, not attributions** (M2 debt): calendar arming already stores the event's attendees. Offer them as naming candidates for unnamed Speakers in that Meeting. Never auto-apply — an invitation is evidence about who was invited, not about who spoke, and applying it would invent attribution
- [ ] De-identification composes from parts that already exist (story 32): deleting a Voiceprint plus renaming the Speaker. No dedicated anonymize mechanism is built, because ADR-0009 rejected one and rename already is it
