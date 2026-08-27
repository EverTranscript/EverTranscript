# Meeting Detection exists and Auto-Record is on by default (amends ADR-0020)

> **Amended 2026-08-27 (continuity window):** auto-stop fires after a short continuity window (~15s default), not instantly on mic release — a Bluetooth-device swap or capture hiccup that re-arms the same trigger inside the window continues the *same* Meeting; the Meeting ends and persists when the window expires. "Record-start to record-stop" is unchanged; the window defines where record-stop is.

> **Amended by ADR-0036:** the local calendar (when granted) pre-arms detection and posts the heads-up at a scheduled meeting's start, and its scheduled end feeds the continuity window. The trigger itself is unchanged — Watchlist AND mic.

Never missing a meeting is the product's headline promise, and ADR-0020's chosen mitigation — a maximally frictionless manual Record — still misses every meeting the Operator forgot. The product therefore gains one ambient sense, Meeting Detection: it observes application and microphone state — never content — to identify an active meeting. Auto-Record, the standing policy that detection starts and stops recording without a per-meeting act, ships **on by default**, and the app installs a launch-at-login item so detection is always running.

What still holds:

- Nothing is captured before the Briefing's explicit acknowledgment. "On by default" means the toggle ships preset to On — not that recording precedes first-run consent education.
- Calendar, screen pixels, filesystem indexing, and contacts remain foreclosed; ADR-0020's other clauses are untouched.
- The always-visible recording indicator (ADR-0007) is now the Operator's moment-to-moment knowledge of capture they didn't initiate — more load-bearing, not less.
- Auto-stop is part of Auto-Record: recording ends when the detected meeting ends, so a Meeting stays record-start-to-record-stop shaped (a capture only a human stops is a capture that runs all day).

## Considered options

A forced onboarding choice (no preselection, "On" badged Recommended — the ADR-0013 pattern) was recommended and rejected: on-by-default was chosen so the product never misses a meeting even for an Operator who breezed through setup. Off-by-default-in-Settings fails the headline promise for exactly the users it was built for; prompt-never-auto-start (the original ADR-0007 allowance) fails it whenever the prompt is missed.

## Consequences

- ADR-0013's invariant — "every configuration the product ever runs traces to an explicit Operator act" — no longer holds globally. Auto-Record traces to the Briefing acknowledgment plus a revocable default, not an affirmative choice. Accepted eyes-open, against recommendation.
- Consent exposure widens: the product records calls the Operator never affirmatively chose to record, including in all-party-consent jurisdictions. The Briefing must state Auto-Record bluntly ("unless you turn this off, this app records your meetings"), and stop/discard must be one action away during any recording.
- The marketing sentence changes shape: from "it hears your meetings when you press Record" to "it never misses a meeting — and nothing it hears ever leaves your machine."
