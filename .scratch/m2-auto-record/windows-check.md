# Windows detection check — 15 minutes on a real machine

Ticket 05's last criterion. The code builds and its whole test suite passes
on `windows-latest` in CI, so it is sound; what CI cannot show is that it
*detects*, because that runner has no microphone and no browsers.

Everything below is copy-paste. Send back the two logs and the answers.

> **Run once, on 2026-08-31 (Windows 11 Pro 26200). Read this before running it again.**
>
> The procedure below could not see the thing it was hunting, and that is
> worth knowing before anyone repeats it. It asks `Get-Process` for the names
> of running processes — but a process existing is not the same as that
> process owning the capture session, and the Core logs an app only *after* a
> Watchlist row matches it. So "no name was read" and "no row was watched"
> produce identical silence.
>
> What was actually wrong was neither: `executable_name` could not name **any**
> process, on any machine, because it asked `GetModuleBaseNameW` over a handle
> lacking the rights that call needs. Windows detection had never detected
> anything. See DECISIONS.md Q27.
>
> **So run the instrument first:**
>
> ```powershell
> cargo run --release -p evertranscript-core --example mic-holders
> ```
>
> It prints every active capture session — the raw executable, the session
> identifier's full path, and what `responsible_app` makes of it — before the
> Watchlist has an opinion. `--pid <n>` asks about one process. That output,
> not the `Get-Process` table, is what settles section 2.
>
> Still unobserved, and the reason this file is kept: **no meeting app has
> ever been seen holding the microphone on Windows.** The machine above had
> only Edge and Teams, and Teams was never driven into a call. If your machine
> has Zoom, VooV, 腾讯会议, or a signed-in Teams, section 2 is still the most
> valuable thing here — and 腾讯会议's executable is still unknown.

## Setup

```powershell
git clone https://github.com/EverTranscript/EverTranscript
cd EverTranscript
cargo build --release -p evertranscript
```

Then, in one terminal, leave the Core running with detection logging on:

```powershell
$env:EVERTRANSCRIPT_LOG = "evertranscript_core=debug"
.\target\release\evertranscript.exe daemon
```

And in a second terminal:

```powershell
.\target\release\evertranscript.exe acknowledge
.\target\release\evertranscript.exe watchlist list
```

**Expect:** Zoom, Microsoft Teams, VooV Meeting, 腾讯会议, Browser Meetings.
If the list is empty or the command errors, stop and send that — the
Watchlist seeds from a migration and an empty list means it did not run.

## 1. Browser Meetings, per browser

For **each** of Chrome, Edge and Firefox that the machine has:

1. Open `https://webrtc.github.io/samples/src/content/getusermedia/audio/`
   and allow the microphone.
2. Within ~10 seconds run `.\target\release\evertranscript.exe status`.

**Expect:** `state Recording`, and in the Core's log a line reading
`Auto-Record started a Meeting ... app="chrome.exe"` (or `msedge.exe`,
`firefox.exe`).

3. Close the tab, wait ~25 seconds, run `status` again.

**Expect:** `state Idle`, and `Auto-Record stopped a Meeting` in the log.

**The thing most likely to be wrong:** the app name. The detector lowercases
the executable, and the Watchlist holds `chrome.exe`, `msedge.exe` and
`firefox.exe` — Brave, Arc and Opera were removed on 2026-08-31
(`DECISIONS.md` Q34), so on this build they are expected *not* to trigger
and a Meeting from one of them would be the surprise. If the log shows a
Meeting attributed to something else — or shows nothing while a browser is
plainly holding the microphone — that is the finding, and the exact string
it reported is what I need.

## 2. A meeting app

With Zoom or Teams installed: open its audio settings and start the
microphone test (no meeting needs to be created). Check `status` within ~10
seconds, then close it and check again after ~25.

**Expect:** the same start/stop pair, attributed to `zoom.exe` or `ms-teams.exe`.

**Please do this step even if step 1 worked, and tell me what it says either
way.** This is now the most valuable thing in this file. On macOS the same
check has failed twice, in exactly the same shape: Safari records under
`com.apple.WebKit.GPU` and Teams under `com.microsoft.teams2.modulehost`, not
under the ids they ship as. Both rows were dead — those apps could never have
triggered — and every unit test passed, because the tests asked about the id I
had assumed. Windows Teams almost certainly does not record from `ms-teams.exe`
either; it has a `msteams.exe`/`ms-teamsupdate.exe` family and I have not seen
which one holds an audio session.

So if nothing starts, **that is the expected result, not a broken setup**, and
the useful output is the name. With the meeting app's microphone test running:

```powershell
Get-Process | Where-Object { $_.ProcessName -match 'teams|zoom' } |
  Select-Object Id, ProcessName, Path | Format-Table -AutoSize
```

Send me that table plus the `status` output. The process name it prints is the
fix.

I have since added a `WINDOWS_EXECUTABLES` table mapping `zoom.exe`,
`ms-teams.exe`, `teams.exe` and `voovmeetingapp.exe` to the rows those apps
ship under — without it, Zoom, Teams and VooV could not match anything on
Windows at all, because their rows hold macOS bundle ids.

I wrote those names from memory first and then checked them against the
equivalent table in Granola's shipped bundle. Three survived. **VooV did
not**: I had written `wemeetapp.exe → com.tencent.meeting`, and it is
`voovmeetingapp.exe → com.tencent.tencentmeeting` — wrong in both halves.
So the table is now checked rather than recalled, which is a real
improvement and still not the same as seeing your machine report them.

Two things I specifically cannot settle from here:

1. **Whether the detector reports these names at all.** That is the whole
   point of the command above.
2. **The Chinese 腾讯会议 build.** Granola's table has only the international
   VooV executable. If you run 腾讯会议 rather than VooV Meeting, its
   executable may differ and I have deliberately not guessed at it — that
   would repeat the exact mistake above. The `Get-Process` output would
   settle it.

## 3. Two Cores at once

This is the one I fixed blind and would most like confirmed:

```powershell
$env:EVERTRANSCRIPT_RUNTIME_DIR = "$env:TEMP\et-a"
.\target\release\evertranscript.exe daemon
```

in a third terminal, while the first Core is still running.

**Expect:** it starts and both answer `status` independently. Before the fix
they shared one pipe and the second could never bind.

## 4. The calendar

```powershell
.\target\release\evertranscript.exe status
```

**Expect** in the log, at startup: `no calendar access; meetings will not be
armed or named in advance`. That is correct and not a failure — the WinRT
appointment store needs package identity, which a binary run from a folder
does not have (recorded on ticket 07). If instead the Core **crashes** at
startup, that is a real finding.

## What to send back

- The Core's full log from both runs.
- The `watchlist list` output.
- For each browser and app: did it start, did it stop, and what `app="..."`
  did the log show.
- Windows version and whether the machine has a real microphone.

A negative result is worth as much as a positive one here — the point is
that nobody has ever watched this run.
