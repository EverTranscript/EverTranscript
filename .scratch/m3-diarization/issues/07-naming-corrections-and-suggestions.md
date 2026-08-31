# 07: Naming as confirmation, corrections as hints, attendees as suggestions

**What to build:** The human-feedback loop — the two ADR amendments this milestone exists to honor, plus the M2 debt that attendee names become suggestions here.

**Blocked by:** 02, 04, 06.

Status: done

- [x] Naming a Speaker **retroactively labels every past appearance** (story 29), because attribution is a live reference and not text baked into the Transcript (ADR-0009). One rename, whole History — including every affected Mirror rebuilt
- [x] **Naming is confirmation** (ADR-0008 as amended): it promotes that Speaker's Voiceprint to an Operator-confirmed tier that wins ties in future matching
- [x] **A correction is an appended hint, never a rewrite** (ADR-0009 as amended, story 29b): re-assigning a segment writes a hint that wins the display join and the Mirrors, while the machine's attribution is preserved beneath it and remains visible to anyone who asks
- [x] Both directions, and it happens **inside** `correct_attribution` rather than being left to the caller — a correction that silently failed to teach anything would look identical to one that worked. The vector re-filed is the exemplar the machine recorded for the wrong Speaker *in that Meeting*: that is the observation which produced the mistake, so it is the one worth moving, and no audio has to be re-opened or model re-run, which is what lets a correction feel instant. Correcting a segment the machine never attributed teaches nobody: that is the Operator filling a gap, not disagreeing, and recording evidence against nobody would be inventing a dispute
- [x] Stored per segment and carried on the wire, with `Operator` as its own basis so a corrected segment says the Operator decided it rather than claiming a Voiceprint match it never had. Original criterion: **visible match attribution** (ADR-0008's mandatory legibility): a Transcript can say why a segment was attributed as it was — matched Voiceprint, channel prior, clustered-only, or Operator correction. An unexplained biometric guess is the thing the ADR bargained against
- [x] **Attendee names become suggestions, not attributions** (M2 debt): calendar arming already stores the event's attendees. Offer them as naming candidates for unnamed Speakers in that Meeting. Never auto-apply — an invitation is evidence about who was invited, not about who spoke, and applying it would invent attribution
- [x] De-identification composes from parts that already exist (story 32): deleting a Voiceprint plus renaming the Speaker. No dedicated anonymize mechanism is built, because ADR-0009 rejected one and rename already is it
