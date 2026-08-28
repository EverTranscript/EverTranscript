# 01: The DetectionSource seam and its fixtures

**What to build:** One trait through which the Core learns what the machine is doing, with a fixture implementation that replays scripted timelines — the M2 twin of AudioSource. Every policy decision in this milestone is tested through this seam; the platform detectors (04, 05) implement it against the real machine.

Status: done

- [x] `DetectionSource` trait producing a stream of detection events: an app became frontmost or exited, a process took or released the microphone, a calendar event started or ended. State only — no window titles, no content
- [x] Events carry the responsible app identity (bundle id / exe), not the raw process: helper-process mapping belongs below the seam so policy never sees a `Google Chrome Helper (Renderer)`
- [x] Events are timestamped on one clock the way capture already is, so a timeline can be replayed deterministically and a test can assert *when* a transition happened
- [x] `FixtureDetectionSource` replays a scripted timeline as fast as the consumer accepts it, with the same completion signal `FixtureSource` uses so tests never sleep-and-hope
- [x] **The fixture can deliver a timeline in fragments, not only whole**: the M1 chunker was correct against whole-file fixtures and discarded every sample from a live microphone. A fixture that only emits tidy, well-spaced events will hide the same class of bug here
- [x] The scripted timelines this milestone needs, as reusable constants: a clean meeting; a mic released for 8 s then re-held (device swap); an app active with no mic; a mic held by a non-Watchlist app; back-to-back meetings in one app; a manual Stop mid-meeting; a calendar-armed meeting that never triggers
- [x] The seam is exercised on both platforms in CI even though the live implementations land later, so the contract cannot drift per-platform (ADR-0025 as amended)
