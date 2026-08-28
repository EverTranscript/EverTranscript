# 09: M2 close-out — the detection matrix, the guarantees, the dogfood proof

**What to build:** The evidence that Auto-Record works, on both platforms, measured rather than asserted. M1's close-out owed a WER number; this one owes false-negative and false-positive rates from real browsers.

**Blocked by:** 03, 04, 05, 06, 07, 08.

Status: ready-for-human — the cycle is proven; the matrix and the rates are not

- [ ] **Chrome only.** Chrome was observed triggering Browser Meetings for real, with its renderer helper correctly attributed. Safari, Arc and Edge are untested, and ADR-0030 is explicit that one browser cannot be extrapolated to the rest — that is why it named this a matrix
- [ ] Not done: Zoom, Teams and VooV were never observed triggering, only the Browser Meetings row
- [x] **Measured, on a small sample and reported as one.** Four live trials against the real detector: three rounds of Chrome taking and releasing the microphone, and one round of an unwatched app holding it. **0 false negatives, 0 false positives.** Trigger latency 4–8 s; auto-stop consistently 20 s, which is the 15 s continuity window plus the release debounce and poll granularity — the design, arriving on time. Four trials is not "a real day of use", and the criterion stays open for a longer run across more apps
- [ ] Not done empirically (covered by unit tests only): the false-trigger blocklist: a dictation app, an IDE with a hot mic, and a screen recorder each observed **not** triggering
- [x] Guarantee tests extended rather than restarted — and they caught a real one: EventKit's default features linked **MapKit and CoreLocation**, now removed and forbidden by name. The zero-network test still passes. (Not yet run *with the calendar granted*, since this machine has no grant.)
- [x] The crash suite covers an auto-started Meeting: one is opened by the detector, the Core is dropped with it still running, and a new Core over the same History recovers the same Meeting: kill mid-recording, kill mid-stop, and confirm the auto-started Meeting recovers exactly as a manual one does
- [x] **The dogfood proof.** Chrome took the microphone on a localhost page; the live detector triggered Browser Meetings; Auto-Record started a Meeting attributed to `com.google.Chrome`; releasing the microphone ran the continuity window out and Auto-Record stopped it. 3m 20s, Mirror written, `Idle` again afterwards, and nobody pressed Record
- [ ] Not done: both platforms green in CI, and the milestone declared done only when they are (ADR-0025 as amended)
