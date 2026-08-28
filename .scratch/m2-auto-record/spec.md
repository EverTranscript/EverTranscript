# M2 — Auto-Record: Meeting Detection, the Watchlist, auto start/stop, calendar arming

Status: ready-for-agent

Sources of truth: `CONTEXT.md` (glossary — its vocabulary is normative here), `docs/prd.md`, ADR-0001–0036 (chiefly 0023, 0024, 0030, 0036, and 0025 as amended), `docs/implementation-notes-2026-08-27.md` (the absorption catalog — its M2 — Detection section has an evidence path for every area below; consult it per the reuse rules in `AGENTS.local.md`). Where this spec and an ADR disagree, the ADR wins.

## Problem Statement

M1 delivered a Core that records well and a record worth trusting — and the Operator still has to press Record. The headline promise is not "it transcribes accurately"; it is **never missing a meeting**, and today the product misses every meeting the Operator forgot, which ADR-0020 already conceded a frictionless manual button cannot fix. Everything M1 proved — dual-channel capture, crash-safe persistence, the tray, the protocol — is machinery waiting for the sense that decides when to use it.

Detection is also the PRD's named product-defining risk: a false negative is a broken headline promise, and the fragile edge (browser meetings via per-process microphone attribution) cannot be settled by reasoning. It has to be built and measured against real browsers on both platforms.

## Solution

The Core grows its ambient senses. A visible, editable **Watchlist** AND live microphone activity trigger recording without a per-meeting act (ADR-0024); a continuity window ends it when the meeting does; a manual Stop wins for the rest of that meeting. The local calendar — read from the OS store only, never a cloud API — pre-arms detection, names the Meeting at birth, and feeds the auto-stop window (ADR-0036), while capture still starts only on the Watchlist-AND-mic trigger: the calendar knows *when*, only the microphone knows *that*.

Detection enters through one seam (**DetectionSource**), the way capture entered through AudioSource in M1, so the policy that decides to record is testable without a meeting, a browser, or a second machine — and the two platform detectors plug into a contract that already has tests.

## User Stories

Numbering follows `docs/prd.md`.

8. As an Operator, I want EverTranscript to start recording by itself when a Watchlist app is in a meeting (app active + microphone in use), so that no meeting is ever missed.
9. As an Operator, I want the watcher to launch at login, so that detection is already running at my first meeting of the day. *(The login item shipped in M1; M2 is what makes it worth having.)*
10. As an Operator, I want recording to stop by itself when the detected meeting ends, so that unattended capture never runs all day.
11. As an Operator, I want my manual Stop to win — detection must not restart recording for the rest of that meeting — so that the machine never overrules me.
12. As an Operator, I want recording to begin mid-meeting when detection comes online during one, so that a partial record beats no record.
13. As an Operator, I want to see and edit the Watchlist, so that I always know exactly what the app watches — and can extend it.
14. As an Operator, I want a single visible Auto-Record switch, so that turning the ambient behavior off (and back on) is one legible act.
16. As an Operator, I want the CLI to carry the new surfaces (Auto-Record switch, Watchlist edits), so that my record stays scriptable.
17. As a Windows Operator, I want detection on Windows 10+ x64 at the same time, so that my platform decides neither my privacy nor my wait (ADR-0025 as amended).
21. As an Operator, I want whole-Meeting delete to be the triage for a recording Auto-Record captured that I never wanted, so that a false positive is one act to undo.
47. As an Operator, I want the app to read my calendar only if I granted it — and to work without it — so that "what does it know?" always has an exact answer.

Two stories the milestone owes that the PRD carries in prose rather than as numbered items: a **heads-up notification** at a scheduled meeting's start (ADR-0036), and the **armed-but-untriggered follow-up** — a calendar-armed Meeting with no trigger by ~2 minutes past start prompts once, and the pre-created Meeting is discarded if ignored.

## Implementation Decisions

- **The DetectionSource seam**: one trait producing a stream of detection events — app became active/inactive, microphone held/released by a process, calendar event started/ended — with a live implementation per platform and a fixture implementation replaying scripted timelines. Every policy test drives the fixture; the platform detectors are tested for what only they can be: that they observe the real machine correctly. This mirrors AudioSource exactly, including the lesson M1 paid for — **fixtures deliver whole timelines, live sources deliver dribbles**, so the policy must be correct under both shapes and the fixture must be able to produce the dribble.
- **The trigger (ADR-0024, unchanged)**: Watchlist membership AND microphone activity, both required. Latching: once triggered, a Meet tab moved to the background keeps recording while the mic stays hot. No pre-roll buffering exists in any form — recording begins at the trigger, and the lost opening seconds are an accepted cost.
- **Auto-stop (ADR-0023 as amended)**: a ~15s continuity window, not an instant stop on mic release, so a Bluetooth swap mid-call continues the *same* Meeting. Prior art gives the refinements: a 500 ms edge debounce at the detector, a 2 s debounce on the mic-holder list going empty, and — at the stop deadline — **re-snapshot which apps hold the microphone and abort the stop if a trigger app re-holds it**.
- **Suppression (story 11)**: a manual Stop suppresses re-trigger for the remainder of that meeting. "That meeting" is the ambiguity to settle in the ticket: the suppression must expire on evidence the meeting ended, not on a timer alone, or the next meeting in the same app is silently missed.
- **The Watchlist (ADR-0030)**: ships Zoom, Microsoft Teams, VooV Meeting (both VooV International and 腾讯会议), and **Browser Meetings** — one entry matching any browser holding a hot microphone, via per-process attribution with helper processes mapped to their responsible app. WeChat ships **suggested, off by default, one tap to add**; membership is the per-app switch, so no per-app toggle column joins ADR-0023's single Auto-Record switch. Stored in the machine store, not the History folder: the Watchlist describes this installation.
- **Detection reads state, never content** — with one honest exception ADR-0036 already names: a calendar event title *is* content, and it is read only under a grant the Operator can decline.
- **macOS detection**: NSWorkspace running-application observation for process entries; per-process microphone attribution via CoreAudio, which serves both the AND-mic condition and Browser Meetings. Re-attach the per-device listener on default-device change, guard stale-device callbacks, and poll as a fallback — detection must survive the same device churn capture already survives. No window titles, no screen content; the opt-in Screen Recording grant for precise browser labels is a Settings affordance, never load-bearing.
- **Windows detection**: Win32 process/window enumeration plus audio-session microphone state, no permission grant required. Granola's `mic_monitor_v2` (backoff, give-up, exe→app table) is the shipped prior art; anarlog's Windows detector is a no-op stub and must not be mistaken for one. **This column cannot be hollow — it is the ship gate.**
- **Helper→responsible-app table**: ported as seed data (~25 Chromium/Electron helper bundle ids plus the Windows exe→id twin). This is the PRD's "fragile edge" pre-solved upstream; porting it beats deriving it.
- **A false-trigger blocklist** ships as negative Watchlist data: dictation apps, IDEs, screen recorders, AI assistants. A hot mic in Cursor is not a meeting.
- **Calendar (ADR-0036)**: the EventKit local store on macOS, the WinRT appointment store on Windows. Never cloud calendar APIs — no OAuth, no new network, Sanctioned Traffic unchanged. Arming pre-creates the Meeting with the event's title and attendees; the title chain becomes **manual > calendar > transcript suggestion > detected-app placeholder**; attendee names surface as Speaker-naming suggestions in M3 and are stored, not applied, now. Access is skippable and Recommended; skipping costs the niceties and nothing else.
- **Notifications**: the heads-up at scheduled start, and the armed-untriggered follow-up. Gated by a cooldown, a per-app silence list, and suppress-while-recording. DND detection is best-effort by construction (the macOS mechanism is undocumented) and must degrade to "notify anyway" rather than to silence — a missed notification is recoverable, a silent product is not.
- **Nothing new in the record**: M2 adds no Transcript semantics. A Meeting born of Auto-Record is a Meeting; the only new columns are those arming needs (calendar event id, title, attendees).

## Testing Decisions

- **Philosophy (unchanged)**: external behavior only. For M2 the observable outputs are the Core's state transitions, the Meetings that exist afterward, and the protocol events Clients saw — not the internals of a detector.
- **The seam is the harness**: `DetectionSource` replaces the real world in every policy test, and this milestone builds the **detection-event fixtures** the PRD promised (scripted timelines: clean meeting, mic released for 8 s then re-held, meeting that never triggers, back-to-back meetings in one app, manual Stop mid-meeting, calendar-armed-then-nothing). These become shared harness for M3 and after.
- **The policy tests that matter are the ones that cost a Meeting**: a Bluetooth swap must not split a Meeting; a manual Stop must not be overruled; the second meeting of the day in the same app must still record; detection coming online mid-meeting must record the remainder.
- **The platform detectors get their own matrix**, and it is empirical: per-browser microphone attribution across Chrome, Safari, Arc and Edge, with helper processes correctly attributed — on both platforms. ADR-0030 named this the M2 test matrix; it replaces locale-sensitive title parsing and cannot be satisfied by unit tests alone.
- **Guarantee tests extend, not restart**: the permission-set audit gains a *conditional* Calendars entry (present only under grant) and must still show no Screen Recording in the default posture; the zero-network test must hold with the calendar granted, which is the assertion that proves "local store, never cloud API".
- **Both platforms, every ticket** (ADR-0025 as amended). A milestone is not done until both pass.

## Out of Scope

- M3: Diarization, Speakers, "You", Voiceprints, attendee names actually applied to Speakers (M2 stores them; M3 suggests from them).
- M4: Summary, Notes, the provider Knob, the transcript-derived title suggestion that sits between calendar and placeholder in the title chain.
- M5: the full Briefing (M1's minimal acknowledgment still stands in), linear onboarding including the calendar step's final copy, the floating mini-indicator, distribution.
- Post-v1 and explicitly not now: prompt-on-unknown-mic-use (ADR-0024 names it a v1.1 candidate), a browser extension with a native-messaging host for precise URL state (ADR-0030 defers it), per-app enable columns beside the single Auto-Record switch.
- Ratified non-goals reaffirmed: pre-roll buffering in any form, capture-at-scheduled-time, cloud calendar APIs, window-title reading, telemetry.

## Further Notes

- **`CONTEXT.md` is stale against ADR-0036 and should be corrected as part of this milestone.** Its Meeting Detection entry still reads "The product's **single** ambient sense" and "It reads state, never content" — both of which ADR-0036 explicitly overturned when the calendar joined and its consequences section reworded the Nothing Ambient clause "honestly — an event title is content". The glossary is normative for this spec, so the drift is load-bearing, not cosmetic.
- The absorption catalog's M2 — Detection section carries an evidence path for every area above: the helper table, the mic-attribution debounces, the stop-side re-snapshot, the blocklist, the notification gates, and the calendar constants. Consult the referenced source before writing new code.
- M1's lesson applies directly and was expensive: the chunker was correct against fixtures and discarded every sample from a live microphone, because fixtures arrive whole and hardware arrives in fragments. A DetectionSource fixture that only ever delivers tidy events will hide exactly the same class of bug in the policy.
- The PRD's risk register puts detection reliability first. The deliverable of this milestone is not only working Auto-Record but **the measured false-negative and false-positive rates from the browser matrix** — a number, in the close-out ticket, the way M1 owed WER.
