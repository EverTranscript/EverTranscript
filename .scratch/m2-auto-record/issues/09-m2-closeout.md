# 09: M2 close-out — the detection matrix, the guarantees, the dogfood proof

**What to build:** The evidence that Auto-Record works, on both platforms, measured rather than asserted. M1's close-out owed a WER number; this one owes false-negative and false-positive rates from real browsers.

**Blocked by:** 03, 04, 05, 06, 07, 08.

Status: ready-for-agent

- [ ] **The per-browser microphone attribution matrix**, on both platforms: Chrome, Safari (macOS), Arc, and Edge — each observed triggering Browser Meetings with helper processes correctly attributed. ADR-0030 named this the M2 test matrix; it cannot be satisfied by unit tests and cannot be extrapolated from one browser to the rest
- [ ] Each shipped Watchlist entry observed triggering and releasing on the platform it belongs to
- [ ] **Measured false-negative and false-positive rates**, reported as numbers, over a real day of use: meetings that should have recorded and did not, and recordings that happened and should not have. This is the deliverable, not a byproduct — detection reliability is the PRD's first-listed risk
- [ ] The false-trigger blocklist earns its place empirically: a dictation app, an IDE with a hot mic, and a screen recorder each observed **not** triggering
- [ ] Guarantee tests extended rather than restarted: the permission audit shows microphone + system audio, Calendars only under grant, and **no Screen Recording in the default posture**; the zero-network test passes with the calendar granted
- [ ] The crash suite still holds with a Meeting that Auto-Record started: kill mid-recording, kill mid-stop, and confirm the auto-started Meeting recovers exactly as a manual one does
- [ ] A dogfood proof: a real meeting the Operator never pressed Record for, recorded end to end, with the record inspected afterwards
- [ ] Both platforms green in CI, and the milestone declared done only when they are (ADR-0025 as amended)
