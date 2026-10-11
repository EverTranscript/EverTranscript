# Packaging

ADR-0016 as amended by ADR-0025: Developer ID signed and notarized on
macOS, a signed installer on Windows, direct download plus a Homebrew cask
and a winget manifest. No app store, so shipping never waits on someone's
opinion of system-audio capture.

## What ships

Three binaries, and they must travel together:

| Binary | What it is |
| --- | --- |
| `EverTranscript.app` (Electron) | The Client — a thin window over the protocol |
| `evertranscript` | The Core: the daemon, the CLI, the tray, the record's only writer |
| `evertranscript-summarizer` | The local Summary sidecar (ADR-0031) |

There was briefly a fourth. ADR-0032 originally specified a bundled ffmpeg,
and nothing ever staged one: the Core looked for it on `PATH`, a developer's
Homebrew build answered on every machine anyone tested on, and a process
spawned by a Finder-launched app — which inherits almost no `PATH` — found
nothing and recorded every Meeting with no audio at all. That is what
reopened the decision. Encoding is now in-process (ADR-0032 as reversed
2026-09-05), so there is no fourth binary to stage, sign, or forget.

The Core is not a helper the Client spawns for convenience: it is the
process that records, and it runs at login without a window. A package that
shipped only the Client would ship a UI for a product that is not there.

## Build

```sh
./packaging/build.sh
```

Produces unsigned artifacts under `packaging/out/`. That is the expected
result on any machine without the certificates below, and it is not a
failure — an unsigned local build is exactly what a contributor should get.
The script says which steps it skipped rather than quietly producing
something that looks signed.

## What only the Operator can do

These need credentials that are deliberately not in this repository:

1. **Developer ID Application certificate** (macOS) — signs every binary in
   the bundle. What must agree across releases is the **team**, not the
   identifiers. The signer passes no `--identifier`, so codesign names the
   Core `evertranscript` and the sidecar `evertranscript-summarizer` after
   their files. That is stable, and the same as a local build's. Ad hoc, it
   would be `evertranscript-<LC_UUID>`, new with every build. The app's
   designated requirement names team `9958A27J47`. Privacy grants are charged
   to the bundle, and Squirrel.Mac installs only an update that satisfies the
   running app's requirement. So a renewed certificate, or a CI-built update,
   keeps every grant and installs. A certificate from another team does
   neither. (Measured 2026-10-10, DECISIONS Q326.)
2. **App Store Connect API key** — notarization. Notarization passes on
   merits; it is Mac App Store *human review* of system-audio capture that
   ADR-0016 rejected, not this.
3. **Windows code-signing certificate** — signs the installer. Without it
   SmartScreen warns on every download.
4. **A tagged release** — the cask and manifest point at release artifacts
   and their checksums, so neither can be generated before one exists.
5. **LGPL-3.0 compliance for the statically linked MP3 encoder.**
   `mp3lame-sys` compiles LAME *into* the Core, so the obligation attaches
   to the binary this product signs and notarizes rather than to a separate
   executable beside it. That is the material difference from the ffmpeg
   sidecar this replaced, which was a child process an Operator could even
   substitute by environment variable: a process boundary makes the
   obligation light, and linking gives that up. It was taken knowingly —
   ADR-0032 records why.

   What that means in practice — provide the LAME source at the version
   linked, and a way for a recipient to relink the Core against their own
   build of it (dynamic linking, or shipping the object files). There is no
   permissive alternative to fall back to: every MP3 encoder reachable from
   Rust is LGPL at the library level, and the `lame` crate's MIT applies to
   its binding code, not to libmp3lame. Whoever signs off on licensing
   should confirm the shape of this before the first release that carries
   it.

## Signing and notarizing releases: the Operator's checklist

Done once, by the Account Holder of team `9958A27J47`. CI then signs and
notarizes every tagged Mac build, and a tagged build fails without these
secrets (DECISIONS Q324). The repository is on github.com, so name it in
full: a bare `EverTranscript/EverTranscript` can resolve to another GitHub
host.

1. **Create the certificate.** In Keychain Access, choose Certificate
   Assistant → Request a Certificate From a Certificate Authority, and save
   the request to disk. On developer.apple.com, open Certificates and add a
   **Developer ID Application** certificate from that request. Download it
   and double-click it, so it lands in your login keychain beside its
   private key.
2. **Export it.** In Keychain Access → My Certificates, export
   "Developer ID Application: Frank Dai (9958A27J47)" as a `.p12` with a
   password.
3. **Create the notarization key.** In App Store Connect → Users and
   Access → Integrations → App Store Connect API, add a Team Key with
   Developer access. Download `AuthKey_<KEYID>.p8`; Apple offers it once.
   Note the Key ID and the Issuer ID shown on that page.
4. **Add the five secrets.** Each value goes from a file or a prompt
   straight into GitHub, never through the clipboard:

   ```sh
   R=github.com/EverTranscript/EverTranscript
   base64 -i DeveloperID.p12 | gh secret set MACOS_CERTIFICATE -R $R
   gh secret set MACOS_CERTIFICATE_PASSWORD -R $R    # prompts for it
   gh secret set APPLE_API_KEY -R $R < AuthKey_<KEYID>.p8
   gh secret set APPLE_API_KEY_ID -R $R --body <KEYID>
   gh secret set APPLE_API_ISSUER -R $R --body <issuer-id>
   gh secret list -R $R                              # five names
   ```

5. **Delete the exported `.p12`.** The certificate stays in your login
   keychain, which is where local builds on this Mac sign from. A lost `.p8`
   is revoked and replaced in App Store Connect, not recovered.

The first tagged build is the proof: its "The package must actually contain
the product" step accepts only an app that Gatekeeper reports as
`Notarized Developer ID` and that is signed by team `9958A27J47`.

### Once per machine: the first signed install is by hand

No installation before the first signed release can update itself into it.
Every earlier Mac release is signed ad hoc, so Squirrel.Mac accepts no other
bundle in its place, and no earlier release has the update strip. Its Core
also predates `update/hold`, so a new Client cannot restart it safely: it
would have no way to keep a recording from starting first (DECISIONS Q333,
Q334). So the first signed install is manual, and it includes quitting the
old Core while it is Idle:

1. **Wait for Idle.** Using the installed app's own Core:

   ```sh
   ET=/Applications/EverTranscript.app/Contents/Resources/evertranscript
   $ET status            # state must be Idle
   $ET diarize status    # must say nothing is being diarized
   ```

   On Windows, the same two commands, with
   `"%LOCALAPPDATA%\Programs\EverTranscript\resources\evertranscript.exe"`.
2. **Quit both halves.** Quit the app. On macOS, then choose **Quit
   EverTranscript** from the menu-bar icon: that stops the Core, which
   quitting the app never does. Windows has no menu-bar Core; there the
   installer stops the old Core itself, so run it only right after step 1.
3. **Install by hand.** Replace the app with the signed release and open it.
   It starts its own Core. On macOS, grant the permissions it asks for
   again: the old grants belong to the old signature.

Every update after that one arrives in the app, and the release notes of the
first signed release say so.

### Rehearsing an update: two betas before a stable release

Stable 1.2.0 is cut only after `v1.2.0-beta.1` → `beta.2` has installed
through the app on the Operator's Mac and on windows-zx8 (DECISIONS Q324).
It is the one test of the whole chain: CI signing, notarization, the GitHub
feed, Squirrel.Mac and NSIS.

**Set the version before each tag.** A tag names a commit and changes no
version. electron-builder names the artifacts and writes `beta-mac.yml` /
`beta.yml` from `clients/electron/package.json`, and the Core reports the
version in `Cargo.toml`. So, for each `v1.2.0-beta.N`:

1. Set `version` in the root `Cargo.toml` (`[workspace.package]`) and in
   `clients/electron/package.json` to `1.2.0-beta.N`: no `v`, the same string
   in both.
2. Run `cargo check`, so `Cargo.lock` follows, and commit the three files as
   `Version 1.2.0-beta.N`, as every earlier version was.
3. Tag that commit `v1.2.0-beta.N`, and publish its release as a
   **pre-release**.

A version left behind fails silently. A feed that still says beta.1 offers no
update, and a Core that still says beta.1 is never restarted, because the
Client restarts only an older Core (Q334). The `-beta.` part puts the build
on the `beta` channel, which is offered later betas and then the stable
release.

**Windows stays unsigned** (Q324, Q13 of the grill). There is no Authenticode
signature to check, SmartScreen warns at the manual beta.1 install, and the
updater's only check is the sha512 in `beta.yml`.

**Evidence, on both machines.** Collect it before the manual beta.1 install,
again before pressing **Restart to Update** for beta.2, and after each
install. `$ET` is the installed Core, as in the first-install steps above.

1. **History backup.**
   - Mac: the rehearsal runs on the real History. With `$ET status` Idle,
     back up the database and check the copy:

     ```sh
     H="$($ET status | awk '/history/ {print $2}')/.data/EverTranscript.db"
     B=~/EverTranscript-backups/EverTranscript.db.before-beta-$(date +%Y%m%d-%H%M%S)
     sqlite3 "$H" ".backup '$B'"
     sqlite3 "$B" 'PRAGMA integrity_check; PRAGMA user_version; SELECT count(*) FROM meetings;'
     ```

   - windows-zx8: the rehearsal never touches the real History. Run it on a
     test History the way Q319 did: start the installer and the app with
     `EVERTRANSCRIPT_HISTORY_DIR`, `EVERTRANSCRIPT_APP_SUPPORT_DIR` and
     `EVERTRANSCRIPT_NO_LOGIN_ITEM` set, untick "Run EverTranscript" on the
     installer's last page, and run every command below with the same
     variables. Record the real History folder's fingerprint (entries, total
     bytes, newest file) before and after.
2. **Release and build identity.** Download the beta's artifact and
   `SHA256SUMS` with `gh release download v1.2.0-beta.N -R
   github.com/EverTranscript/EverTranscript`, and check the artifact's hash
   against its line.
   - Mac: in the unzipped release, `dwarfdump --uuid
     EverTranscript.app/Contents/Resources/evertranscript` gives the
     `LC_UUID`, and the installed Core must give the same one. `spctl -a -vv
     /Applications/EverTranscript.app` must say `Notarized Developer ID`, and
     `codesign -dv` must show `TeamIdentifier=9958A27J47`.
   - windows-zx8: before **Restart to Update**, `certutil -hashfile` of
     `%LOCALAPPDATA%\@evertranscriptclient-updater\pending\EverTranscript-Setup-1.2.0-beta.2.exe`
     must match that file's line in `SHA256SUMS`.
3. **The running Core's version.** After each install, `$ET --version` (the
   binary on disk) and `$ET status` (the Core that is running, with its pid)
   must both name the beta. A `status` that still names the previous version,
   under the old pid, means the Core was not restarted (Q333, Q334). On
   zx8, `status` must also still name the test History.
4. **History compared.** After each install, `$ET list` names the same
   Meetings as before it. On the Mac, the database passes `integrity_check`,
   and its Meeting count and `user_version` match the backup, unless the beta
   adds a migration. On zx8, the real History's fingerprint is unchanged.

## Artifact names are load-bearing

The in-app updater downloads by filename. electron-builder writes
`latest.yml` / `latest-mac.yml` naming each artifact, electron-updater asks
the newest GitHub release for exactly that name, and **GitHub rewrites spaces
in uploaded asset names**. Those three facts have to agree.

v1.0.0 shipped before they did. NSIS's default `artifactName` is
`${productName} Setup ${version}.${ext}` — with spaces — so one file had
three names: `EverTranscript Setup 1.0.0.exe` on disk,
`EverTranscript-Setup-1.0.0.exe` in `latest.yml`, and
`EverTranscript.Setup.1.0.0.exe` once GitHub had it. Windows updates would
have 404'd. The assets were renamed by hand after publishing; macOS was never
affected, because its name has no spaces to rewrite.

So `nsis.artifactName` is pinned to the hyphenated form, and the packaging
job asserts the agreement rather than trusting it: every name in every
channel's feed (`latest*.yml`, or `beta*.yml` for a pre-release) must exist
among the built artifacts, and no artifact may carry a space. **Any new target — dmg, msi, appimage — needs the same treatment**,
because the default names for several of them also contain spaces.

## Entitlements

`macos/entitlements.plist` carries what the product actually needs and
nothing else. The guarantee suite already forbids ScreenCaptureKit,
Contacts, MapKit and CoreLocation in the binary; the entitlements file is
where that becomes checkable by someone who has only the download
(story 46). Two of those four were linked by accident through a
dependency's default features in M2 and found by that audit.

## Uninstall

Removing the app removes the app. **The History folder is deliberately left
behind** (ADR-0035): it is the Operator's record and the complete portable
unit, and an uninstaller that deleted someone's meeting archive because they
were switching machines would be the most destructive thing this product
could do. The cask and manifest say so rather than inheriting a default.
