# Signed in-app updates

Status: ready-for-agent

Settles `.scratch/zx8-upgrade-e2e/issues/03-the-in-app-updater-never-offers-an-update.md`:
build the in-app offer, and sign and notarize the macOS app with the Operator's
paid Apple Developer team, `9958A27J47`. Agreed with the Operator in a grilling
session on 2026-10-10 (Q1–Q17 below).

## Facts the decisions rest on

- No release so far is signed. v1.0.1, v1.1.1 and v1.1.2 are ad hoc, and each
  one's designated requirement is its own `cdhash`. Squirrel.Mac installs an
  update only if it satisfies the running app's designated requirement, so
  none of them can ever update in-app.
- This Mac runs a local build signed with *Apple Development: Frank Dai
  (CCDB33UUQ9)*. Its requirement names that one certificate, so it would also
  refuse CI-signed updates.
- Xcode records team `9958A27J47` as a paid Individual team. No Developer ID
  identity exists yet, and no GitHub secret is set.
- `package.yml` already reads `MACOS_CERTIFICATE`, `MACOS_CERTIFICATE_PASSWORD`,
  `APPLE_API_KEY`, `APPLE_API_KEY_ID` and `APPLE_API_ISSUER`. It writes the
  secret's value into `APPLE_API_KEY`, but `@electron/notarize` expects a path
  to the `.p8` file.
- On Windows the NSIS updater stops every process under the install directory,
  the Core included, so an update kills a recording.
- On macOS the Core runs from a LaunchAgent and survives the bundle swap, still
  running the old binary.
- An unsigned NSIS app has no `publisherName`, so it accepts an unsigned
  installer. Its only check is the sha512 in `latest.yml`.
- electron-updater 6.8.9: a pre-release build turns on `allowPrerelease`. A
  `beta` build is offered later betas and the next stable; an `rc` build is
  only offered more `rc` builds.

## Decisions

| # | Decision |
|---|---|
| Q1 | CI signs with a *Developer ID Application* certificate kept in GitHub secrets. Assembling the release stays manual. |
| Q2 | CI notarizes. No agent runs `notarytool` by hand. |
| Q3 | The Operator does the Apple-side steps from the checklist in `packaging/README.md`. |
| Q4 | No install while the Core is recording or diarizing. An install that is due waits for Idle. |
| Q5 | Work lands on `main`. The Operator cuts every release. |
| Q6 | Windows signing is out of scope. |
| Q7 | This Mac moves to Developer ID too, at the cost of one re-grant of its five permissions. |
| Q8 | Existing installs need one manual install of the first signed release; its notes say so. |
| Q9 | One strip in the main window: Download / Release notes / Later → progress → Restart to update. No menu item, notification or tray item. |
| Q10 | Check at start, then every 24 hours. |
| Q11 | Install only on an explicit click. `autoInstallOnAppQuit` is off. |
| Q12 | The new Client restarts an older Core once it is Idle (`launchctl kickstart` on macOS). |
| Q13 | The offer ships on Windows too, unsigned, behind the same gate. |
| Q14 | Amend ADR-0034 to cover the download; correct "nothing listens on a socket" in the entitlements comment and on What It Knows. |
| Q15 | Rehearse `v1.2.0-beta.1` → `beta.2` on this Mac and on windows-zx8 before stable 1.2.0. |
| Q16 | A `v*` tag build fails without signing and notarization secrets. |
| Q17 | Background check errors stay silent. A failed Download or Restart shows in the strip with Try again. |

## Slices

Each lands on `main` after fmt, clippy, the Rust suite and the Electron tests.

1. **CI and signing.** Write the `.p8` to a file for `APPLE_API_KEY`. Fail a
   tag build without secrets. Upload every channel `*.yml`. Confirm the Core's
   codesign identifier under Developer ID. Put the Operator's checklist in
   `packaging/README.md`.
2. **Updater in main.** Checks at start and every 24 hours, no install on quit,
   events forwarded to the window, and the Q4 gate enforced in main.
3. **Preload bridge and the strip,** English and Chinese, with Q17 errors.
4. **Core catch-up** (Q12).
5. **Trust surface** (Q14).
6. **This Mac's local signing** moves to Developer ID once the certificate
   exists.

## The Operator's part

1. Create a Developer ID Application certificate and export it as `.p12`.
2. Create an App Store Connect API key: the `.p8`, its Key ID and Issuer ID.
3. Add the five secrets named above.
4. Cut `v1.2.0-beta.1`, then `beta.2`, then stable once both rehearsals pass.
5. Re-grant the five permissions on this Mac when beta.1 is installed.

## Done when

- beta.1 → beta.2 installs in-app on this Mac: the new `LC_UUID` matches the
  release, the Core runs the new version, and History matches a backup.
- The same holds on windows-zx8.

## Out of scope

Windows signing, the Mac App Store, a dmg, bridging the ad hoc versions, and
the LGPL item in `packaging/README.md`.
