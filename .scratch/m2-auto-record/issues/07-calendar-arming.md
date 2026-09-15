# 07: Calendar arming and event titles

**What to build:** The second ambient sense (ADR-0036), read from the OS store only: at a scheduled meeting's start the Core pre-arms detection, pre-creates the Meeting with the event's title and attendees, and feeds the scheduled end into the auto-stop window. **Capture still starts only on the Watchlist-AND-mic trigger** — the calendar knows *when*, only the microphone knows *that*.

**Blocked by:** 03.

Status: blocked — the Windows reader works under a signed test package (2026-09-15). Still open: the scheduled end is not wired, the Mac reader has never read a real calendar, and whether Windows keeps a local-store reader at all is escalated (`DECISIONS.md` Q104)

- [x] EventKit local-store read on macOS — written, and it compiles and links correctly.
- [x] **The WinRT appointment store is read** — `AppointmentManager` with read-only access to all calendars. Run on Windows 11 Pro 26200 on 2026-09-15 under a signed sparse test package; see *Run on Windows* below
- [x] **Never a cloud calendar API**: no OAuth, no token lifecycle, no new network. The zero-network guarantee test must still pass with the calendar granted — that test is what proves this clause rather than asserting it
- [x] Access is a skippable, Recommended step; an Operator who declines gets the whole product minus the niceties, and no feature silently degrades beyond the calendar ones
- [x] Arming pre-creates the Meeting carrying the event's title and attendees; an ignored armed Meeting is discarded (with 06's follow-up), never left as an empty row. *As built, arming creates no row:* a recording that starts while a meeting is armed takes its title and attendees, so an ignored one leaves nothing to discard. `Action::ArmedMeetingNeverStarted`'s doc still speaks of a pre-created Meeting
- [x] Title chain becomes **manual > calendar event title > transcript suggestion > detected-app placeholder** (ADR-0030 as amended). The transcript-suggestion link lands in M4; M2 must leave the seam for it rather than hard-coding a two-step chain
- [x] The event id and title reach Mirror frontmatter (`calendar_event:` and `invited:`); attendee names are **stored, not applied** — rendered as who was invited and never as who spoke, which is M3's question and needs Diarization to answer
- [ ] Scheduled end feeds the continuity window, with the early-end and end-grace constants taken from the prior art rather than guessed. **Not done, though this box was ticked.** Every `CalendarEvent` carries `scheduled_end` and nothing reads it: the continuity window never sees it, and there are no early-end or end-grace constants. Found 2026-09-15
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

## Not verified on the Mac

The macOS reader compiles and links, and asking for the authorization status
works — but this machine has **no Calendars grant**, so no event has ever
been read and no Meeting has ever been armed by a real calendar. Arming
after a meeting's start rather than an hour before it is covered by
`changes`' tests, and the policy side end to end by fixtures
(`auto_record.rs`). Both are weaker claims than a real calendar. Granting
access and watching a scheduled meeting arm and name a Meeting is what
closes this.

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
