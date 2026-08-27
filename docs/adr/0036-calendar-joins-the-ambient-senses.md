# The local calendar joins the ambient senses: it arms and names, never triggers

Overturning ADR-0020's calendar foreclosure (2026-08-27, eyes-open): EverTranscript reads the **local calendar store** — EventKit on macOS, which already carries Google/Outlook calendars synced through Internet Accounts; the WinRT appointment store on Windows (fast-follow homework) — and **never cloud calendar APIs**: no OAuth, no new network, Sanctioned Traffic (ADR-0034) unchanged. The role is bounded: at a scheduled meeting's start the Core pre-arms detection, posts the heads-up notification, and pre-creates the Meeting carrying the event's title and attendees; **capture still starts only on ADR-0024's Watchlist-AND-mic trigger** — the calendar knows *when*, only the microphone knows *that* — and the scheduled end feeds the auto-stop continuity window. Access is a **skippable, Recommended onboarding step** (the ADR-0013 pattern): Operators who skip it get the full product minus calendar niceties, and for them story 47's original sentence stays literally true. Title priority becomes **manual > calendar event title > transcript suggestion > detected-app placeholder** (the filename slug follows, ADR-0035); attendee names surface as suggestions when naming Speakers — never auto-applied; the event id and title land in Mirror frontmatter.

## Considered options

Cloud calendar APIs (anarlog's OAuth path — rejected: credentials, a token lifecycle, and two Sanctioned Traffic entries for calendars the OS store already carries). Capture-at-scheduled-time (anarlog ships it — rejected: it records room audio and silence when the Operator skips or joins late, the wiretap shape ADR-0024 exists to prevent). Keeping the foreclosure (the 2026-07 position, twice reaffirmed — overturned because two of three competitors prove the value in source, advance knowledge of scheduled meetings strengthens the never-miss promise, and titles arrive at the Meeting's birth instead of after its Summary).

## Consequences

- Nothing Ambient narrows a second time, and its "reads state, never content" clause is reworded honestly — an event title is content. Story 47 becomes tiered (granted vs skipped).
- The permission sheet gains an optional Calendars prompt; the Nothing-Ambient audit's expected set gains a conditional entry.
- The Briefing and the mandatory counsel review gain a calendar clause.
- Windows appointment-store access lands in M2 beside per-process mic attribution (ADR-0025 as amended: simultaneous v1).
