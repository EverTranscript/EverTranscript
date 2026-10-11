# 03: The in-app updater never offers an update

Status: ready-for-human

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

## Comments

**2026-10-10.** Decided: build the offer, and sign and notarize the Mac app.
The plan is `.scratch/signed-in-app-updates/spec.md` (DECISIONS Q324).

One correction to "What the code does": the check does not run either.
`maybeCheckForUpdates` sends `settings/get` on a connection it never
initialized, the Core refuses it with `NOT_INITIALIZED`, and the `catch`
treats that as "no Core". So no release from v1.0.1 to v1.1.2 has asked
GitHub for an update (DECISIONS Q328). Slice 2 fixes it.

**2026-10-10, later.** The offer is built and on `main` (Q325–Q337):

- CI signs and notarizes a tagged Mac build, or the build fails (8c8f210).
- The Client checks at start and every 24 hours, downloads only on Download,
  and installs only on Restart to Update (1f0f2e1).
- The update strip in the main window, in English and Chinese (cc831ce).
- The install waits for a hold from the Core, which lasts for as long as the
  installer runs; an older Core is restarted under that hold (24a7edb,
  verified on Windows CI in Q337).
- What It Knows names both hosts an update reaches (3e928b2).

What remains is the Operator's: the Developer ID certificate, the
notarization key and the five secrets (`packaging/README.md`, "the
Operator's checklist"), then the `v1.2.0-beta.1` → `beta.2` rehearsal on
this Mac and on windows-zx8 (Q15). No update has yet been installed through
the offer. Each machine's first signed install is by hand.
