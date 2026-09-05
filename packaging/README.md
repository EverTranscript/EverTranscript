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

1. **Developer ID Application certificate** (macOS) — signs all four
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
