# 07: Calendar arming and event titles

**What to build:** The second ambient sense (ADR-0036), read from the OS store only: at a scheduled meeting's start the Core pre-arms detection, pre-creates the Meeting with the event's title and attendees, and feeds the scheduled end into the auto-stop window. **Capture still starts only on the Watchlist-AND-mic trigger** — the calendar knows *when*, only the microphone knows *that*.

**Blocked by:** 03.

Status: blocked — both readers written; needs a Calendars grant and a Windows machine

- [x] EventKit local-store read on macOS — written, and it compiles and links correctly.
- [x] **The WinRT appointment store is written** — `AppointmentManager` with read-only access to all calendars, typechecked against `x86_64-pc-windows-msvc` and never executed, the same status as the Windows detector
- [x] **Never a cloud calendar API**: no OAuth, no token lifecycle, no new network. The zero-network guarantee test must still pass with the calendar granted — that test is what proves this clause rather than asserting it
- [x] Access is a skippable, Recommended step; an Operator who declines gets the whole product minus the niceties, and no feature silently degrades beyond the calendar ones
- [x] Arming pre-creates the Meeting carrying the event's title and attendees; an ignored armed Meeting is discarded (with 06's follow-up), never left as an empty row
- [x] Title chain becomes **manual > calendar event title > transcript suggestion > detected-app placeholder** (ADR-0030 as amended). The transcript-suggestion link lands in M4; M2 must leave the seam for it rather than hard-coding a two-step chain
- [x] The event id and title reach Mirror frontmatter (`calendar_event:` and `invited:`); attendee names are **stored, not applied** — rendered as who was invited and never as who spoke, which is M3's question and needs Diarization to answer
- [x] Scheduled end feeds the continuity window, with the early-end and end-grace constants taken from the prior art rather than guessed
- [x] **Done ahead of this ticket.** `CONTEXT.md` was stale against ADR-0036 and is corrected: Meeting Detection no longer claims to be "the product's **single** ambient sense", and Calendar Arming is defined beside it as the second one. The drift was internal — the Nothing Ambient entry already enumerated both senses, so the two entries contradicted each other. "Reads state, never content" is kept on Meeting Detection, where it is still true; the calendar entry is where the honest exception now lives
- [x] The permission-set audit was updated, and it earned its keep: adding EventKit with default features linked **MapKit and CoreLocation**, which `default-features = false` removed and the guarantee now forbids by name. Original criterion: Calendars as a **conditional** entry: present under grant, absent otherwise, and Screen Recording still absent in the default posture

## Not verified on this machine

The macOS reader compiles and links, and asking for the authorization status
works — but this machine has **no Calendars grant**, so no event has ever
been read and no Meeting has ever been armed by a real calendar. The policy
side is covered end to end by fixtures (`auto_record.rs`), which is a
different claim and a weaker one. Granting access and watching a scheduled
meeting arm and name a Meeting is what closes this.
