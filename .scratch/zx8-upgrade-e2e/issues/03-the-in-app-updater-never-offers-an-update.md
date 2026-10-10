# 03: The in-app updater never offers an update

Status: needs-triage

Priority: medium. People on 1.0.1 are not told that 1.1.1 exists, and the
same will be true for 1.1.1 users when a later release ships.

Found preparing the 1.0.1 → 1.1.1 upgrade on windows-zx8 on 2026-10-10. The
code is the same in v1.0.1, v1.1.1 and `main` (d6ddd18).

## What the code does

- With "Check for updates" on (the default), `startUpdateChecks`
  (`updates.ts:32`) calls `checkForUpdates()` (`updates.ts:48`).
- `autoDownload` is `false` (`updates.ts:39`), so a found update is not
  downloaded.
- The main process handles `updates:download` and `updates:install`
  (`index.ts:653`, `index.ts:657`), but nothing calls them: no preload bridge,
  no renderer code, and no listener for `update-available`.
- So the check runs, finds the new version, and stops. Nothing is shown.

## Decision needed

- **Build the offer:** show that an update is available, download it, and
  restart to install. This also needs signing, because neither installer is
  signed yet.
- **Or drop the setting** until that exists, so the app does not suggest it
  updates itself.

The 1.1.1 release notes say no update has yet been installed through the
in-app updater. That is true, but it reads as if one could be.
