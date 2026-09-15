# 07: Calendar arming and event titles

**What to build:** The second ambient sense (ADR-0036), read from the OS store only: at a scheduled meeting's start the Core pre-arms detection, pre-creates the Meeting with the event's title and attendees, and feeds the scheduled end into the auto-stop window. **Capture still starts only on the Watchlist-AND-mic trigger** — the calendar knows *when*, only the microphone knows *that*.

**Blocked by:** 03.

Status: blocked — the Windows reader works under a signed test package and the Mac reader arms from a real calendar (2026-09-15, Q108 made the app able to ask). Still open: a Meeting named by an armed event has not been observed under capture, and whether Windows keeps a local-store reader at all is escalated (`DECISIONS.md` Q104)

- [x] EventKit local-store read on macOS — written, and it compiles and links correctly.
- [x] **The WinRT appointment store is read** — `AppointmentManager` with read-only access to all calendars. Run on Windows 11 Pro 26200 on 2026-09-15 under a signed sparse test package; see *Run on Windows* below
- [x] **Never a cloud calendar API**: no OAuth, no token lifecycle, no new network. The zero-network guarantee test must still pass with the calendar granted — that test is what proves this clause rather than asserting it
- [x] Access is a skippable, Recommended step; an Operator who declines gets the whole product minus the niceties, and no feature silently degrades beyond the calendar ones
- [x] Arming pre-creates the Meeting carrying the event's title and attendees; an ignored armed Meeting is discarded (with 06's follow-up), never left as an empty row. *As built, arming creates no row:* a recording that starts while a meeting is armed takes its title and attendees, so an ignored one leaves nothing to discard
- [x] Title chain becomes **manual > calendar event title > transcript suggestion > detected-app placeholder** (ADR-0030 as amended). The transcript-suggestion link lands in M4; M2 must leave the seam for it rather than hard-coding a two-step chain
- [x] The event id and title reach Mirror frontmatter (`calendar_event:` and `invited:`); attendee names are **stored, not applied** — rendered as who was invited and never as who spoke, which is M3's question and needs Diarization to answer
- [x] Scheduled end feeds the continuity window, with the early-end and end-grace constants taken from the prior art rather than guessed. *Ticked once before it was true; built 2026-09-15.* It follows anarlog's rule: a browser meeting named by a calendar event that goes quiet more than 3 minutes before its scheduled end (`AUTO_STOP_CALENDAR_EARLY_END_THRESHOLD_MS`) gets a 45 s window instead of 15 s. The extra 30 s is how long anarlog's "Did your meeting end?" prompt waited before stopping. Every other quiet keeps 15 s. anarlog's 10-minute end grace is not used: it holds a meeting only through a network outage, which nothing here can sense. anarlog had also tried holding browser meetings until the scheduled end, up to 10 minutes, and replaced that with the prompt (`DECISIONS.md` Q106). Checked by `a_browser_meeting_that_goes_quiet_early_has_longer_to_come_back`
- [x] **Done ahead of this ticket.** `CONTEXT.md` was stale against ADR-0036 and is corrected: Meeting Detection no longer claims to be "the product's **single** ambient sense", and Calendar Arming is defined beside it as the second one. The drift was internal — the Nothing Ambient entry already enumerated both senses, so the two entries contradicted each other. "Reads state, never content" is kept on Meeting Detection, where it is still true; the calendar entry is where the honest exception now lives
- [x] The permission-set audit was updated, and it earned its keep: adding EventKit with default features linked **MapKit and CoreLocation**, which `default-features = false` removed and the guarantee now forbids by name. Original criterion: Calendars as a **conditional** entry: present under grant, absent otherwise, and Screen Recording still absent in the default posture

## Fixed before the Windows run (2026-09-15)

Reading the code before testing it found bugs that would have made any test
meaningless. They are fixed, with tests (`DECISIONS.md` Q101, Q102):

- The source announced every event a reader returned, and both readers
  returned events up to an hour ahead. A meeting armed when it was first
  seen, its "never started" follow-up came two minutes later, and its title
  went to whatever Auto-Record started in that hour. `changes` now announces
  a meeting once its start has passed and its end has not, and announces the
  end once it is over or gone from the store.
- The Windows reader queried the hour after 1 January 1601, took an
  appointment's length for its end, asked for no properties (a WinRT query
  loads almost none it is not asked for) and read no invitees.
- All-day entries armed at midnight. They are skipped now, as anarlog skips
  them.

## Run on Windows (2026-09-15)

On windows-zx8 (Windows 11 Pro 26200), a test Core built from the fix got
package identity from a signed sparse package declaring `appointments`. It
ran with Auto-Record on over an empty Watchlist, so it could not record. A
test binary under the same package added four appointments to a calendar of
its own:

| Appointment | Stored start (UTC) | Armed |
|---|---|---|
| In progress when the Core started | 07:46 | 07:56:46, on the first poll |
| Starting about five minutes in | 08:01 | 08:01:16, the first poll after its start |
| Later that day | 08:36 | no; it had not started when the run ended |
| All day | midnight local | no |

Also measured:

- The store matches a range by **overlap**: a two-minute query around now
  returned the appointment already in progress.
- It keeps start times **to the whole minute**. An appointment created for
  08:01:28 came back as 08:01, which is why it armed at 08:01:16.
- Invitees read back by display name.
- The `appointments` capability was enough for read-only access to all
  calendars.

Not observed:

- A Meeting named by an appointment. Naming happens when a recording starts,
  and the test Core could not record.
- The end of an armed meeting. The Core logs nothing when one ends. The
  store did read the second appointment back as over, and `changes` is
  tested on exactly that input.
- The "never started" follow-up, because the notifier is silent.

Everything was removed afterwards, and the machine compared equal to a
snapshot taken before the run (`DECISIONS.md` Q103).

Since the run, a query that fails skips that poll instead of reading as an
empty store (`DECISIONS.md` Q107). Only those failure branches changed. They
typecheck for Windows but have not run there. While the store stays
unreadable, a meeting already armed still ends at its latest known scheduled
end (Q110), so it cannot name a recording hours later.

## What the store holds on Windows 11

Before the test, zx8's appointment store held one calendar, the default
local one, with no appointments in the 30 days either side. That machine
has the new Outlook and not Mail and Calendar, the app whose accounts filled
this store. Microsoft retired Mail and Calendar at the end of 2024, and the
new Outlook is reported not to fill the store. None of Granola, anarlog or
Meetily reads it; anarlog and Granola use cloud calendars on Windows. So on
a current Windows 11 machine the reader works but will probably find
nothing. Whether ADR-0036's Windows half stays a local-store reader is
escalated as `DECISIONS.md` Q104.

## Run on the Mac (2026-09-15)

Verified against a real calendar on macOS 26.6.2. A release Core with
scratch History, runtime and support dirs, `autoRecord: true` and an
**emptied Watchlist** (so it could arm but never record) was started from a
terminal that holds the Calendars grant. An event created in Calendar.app
two minutes into its ten-minute slot was announced on the next poll:
`a scheduled meeting has started` and `armed by the calendar … title="…"`
within 30 s; deleting it ended the arming; no Meeting row was created. Same
result on `b4af2f7` and `5a980f9`.

The installed app could not be granted at all until Q108: nothing asked,
the Client had no usage string and the hardened runtime no Calendars
entitlement, so macOS never listed it under Privacy & Security. Now
`calendar/requestAccess` asks from the Core, the onboarding step and the
trust surface carry the button, and `evertranscript calendar request` does
it from a terminal. Frank granted it on 2026-09-15, and the grant covers
the login-item Core too, which an earlier draft of this note denied:
probed afterwards, the bundle's own Core binary started as a launchd job
(parent pid 1) read `calendarGranted: true`, and a copy of the same binary
outside the bundle read `false`. TCC keys the grant on the app bundle, not
on which process spawned the Core.

Still never observed: a Meeting *named* by an appointment, which needs a
real capture during an armed event.

## A Windows constraint worth knowing before anyone relies on this

`AppointmentManager` requires **package identity**. A binary run from a
folder — which is what a `cargo` build, a CI runner, and a plain download
all are — has none, and the API does not politely refuse: the Windows CI
test binary exited abnormally. The reader now asks
`GetCurrentPackageFullName` first and reports the calendar as unavailable
when there is no package, which is the truthful answer rather than a
workaround.

The consequence for shipping is real and belongs to M5's distribution work:
**calendar arming on Windows needs package identity.** An unpackaged
Windows install gets the whole product minus calendar arming, which is the
same posture ADR-0036 already gives an Operator who declines the grant — so
nothing else has to change to accommodate it.

A full MSIX build is not the only way to get identity. The 2026-09-15 run
used a **sparse package**: the exe keeps running from its own folder, carries
an embedded manifest naming the package, and the installer registers a signed
package that points at that folder (`Add-AppxPackage -ExternalLocation`).
That fits the NSIS installer. Shipping it would need a code-signing
certificate the machine already trusts, plus registering the package at
install and removing it at uninstall.
