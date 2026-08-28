# 01: Asset pipeline

**What to build:** A `brand/` workspace package whose `render.mjs` turns SVG masters into every platform format without any tool that is not already in the repo's toolchain: `@resvg/resvg-js` rasterizes; `.icns`, `.ico` and the menu bar's multi-representation `.tiff` are written by hand (their containers are a few dozen lines each); `Contents.json`, the Android adaptive-icon XML and `site.webmanifest` are emitted. Text is never rendered (`loadSystemFonts: false`) so the output is byte-identical on every machine.

**Blocked by:** —

**Status:** resolved

- Directory names matter: `.gitignore:76` (`Icon?`, with `core.ignorecase = true`) silently ignores any path segment named `icons`, and `dist/`, `build/`, `out/` are ignored too. Masters live in `brand/src/`, outputs in `brand/generated/`, tray glyphs in `crates/evertranscript-core/src/tray/glyphs/`, Electron copies flat in `clients/electron/resources/`. A `!**/icons/` negation is added so the trap cannot bite a future directory.
- The script is named `render`, not `build`, so the root `pnpm -r build` fan-out never regenerates assets in CI.
