# 03: The Auto-Record policy — trigger, latch, continuity, suppression

**What to build:** The decision itself, built entirely against the DetectionSource seam and testable without a meeting, a browser, or a second machine. This is where "never miss a meeting" either holds or does not.

**Blocked by:** 01, 02.

Status: ready-for-agent

- [ ] The trigger is Watchlist membership **AND** microphone activity, both required (ADR-0024): an idle Zoom window records nothing, and a hot mic alone — dictation, a voice memo — triggers nothing
- [ ] Detection latches once triggered: a Meet tab moved to the background keeps recording while the mic stays hot
- [ ] Recording begins at the trigger with **no pre-roll buffering in any form** (ADR-0024 as amended) — not a ring buffer, not "just a few seconds". The lost opening is an accepted cost
- [ ] Auto-stop fires after a ~15 s continuity window rather than instantly on mic release, so a Bluetooth swap mid-call continues the **same** Meeting rather than splitting it (ADR-0023 as amended)
- [ ] At the stop deadline, re-snapshot which apps hold the microphone and **abort the stop if a trigger app re-holds it**; a re-trigger inside the window cancels the pending stop outright
- [ ] Debounces from the prior art rather than invented: ~500 ms at the detector edge, ~2 s on the mic-holder list going empty
- [ ] A manual Stop suppresses re-trigger for the rest of that meeting (story 11). **Settle what ends the suppression** — it must expire on evidence the meeting ended, not on a timer alone, or the day's second meeting in the same app is silently missed. Whatever is chosen, write down why
- [ ] Detection coming online mid-meeting starts recording the remainder (story 12): a partial record beats no record
- [ ] Auto-Record off means the policy decides nothing at all, and the single switch is the only thing that has to be turned to stop it (ADR-0023)
- [ ] Nothing is captured before the Briefing acknowledgment, unchanged from M1: the pre-capture invariant is not weakened by an ambient trigger
- [ ] Every criterion above has a test driven from a scripted timeline through the seam — including the two that cost a Meeting when wrong: the device swap that must not split, and the manual Stop that must not be overruled
