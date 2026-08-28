# 07: Calendar arming and event titles

**What to build:** The second ambient sense (ADR-0036), read from the OS store only: at a scheduled meeting's start the Core pre-arms detection, pre-creates the Meeting with the event's title and attendees, and feeds the scheduled end into the auto-stop window. **Capture still starts only on the Watchlist-AND-mic trigger** — the calendar knows *when*, only the microphone knows *that*.

**Blocked by:** 03.

Status: ready-for-agent

- [ ] EventKit local-store read on macOS; the WinRT appointment store on Windows, in this milestone, not after it (ADR-0025 as amended)
- [ ] **Never a cloud calendar API**: no OAuth, no token lifecycle, no new network. The zero-network guarantee test must still pass with the calendar granted — that test is what proves this clause rather than asserting it
- [ ] Access is a skippable, Recommended step; an Operator who declines gets the whole product minus the niceties, and no feature silently degrades beyond the calendar ones
- [ ] Arming pre-creates the Meeting carrying the event's title and attendees; an ignored armed Meeting is discarded (with 06's follow-up), never left as an empty row
- [ ] Title chain becomes **manual > calendar event title > transcript suggestion > detected-app placeholder** (ADR-0030 as amended). The transcript-suggestion link lands in M4; M2 must leave the seam for it rather than hard-coding a two-step chain
- [ ] The event id and title reach Mirror frontmatter; attendee names are **stored, not applied** — they become Speaker-naming suggestions in M3
- [ ] Scheduled end feeds the continuity window, with the early-end and end-grace constants taken from the prior art rather than guessed
- [x] **Done ahead of this ticket.** `CONTEXT.md` was stale against ADR-0036 and is corrected: Meeting Detection no longer claims to be "the product's **single** ambient sense", and Calendar Arming is defined beside it as the second one. The drift was internal — the Nothing Ambient entry already enumerated both senses, so the two entries contradicted each other. "Reads state, never content" is kept on Meeting Detection, where it is still true; the calendar entry is where the honest exception now lives
- [ ] The permission-set audit gains Calendars as a **conditional** entry: present under grant, absent otherwise, and Screen Recording still absent in the default posture
