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

1. **Developer ID Application certificate** (macOS) — signs all three
   binaries. Their codesigning identifiers must agree, or macOS refuses to
   launch the child processes; the failure presents as the Core "not
   starting", with nothing in any log.
2. **App Store Connect API key** — notarization. Notarization passes on
   merits; it is Mac App Store *human review* of system-audio capture that
   ADR-0016 rejected, not this.
3. **Windows code-signing certificate** — signs the installer. Without it
   SmartScreen warns on every download.
4. **A tagged release** — the cask and manifest point at release artifacts
   and their checksums, so neither can be generated before one exists.

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
