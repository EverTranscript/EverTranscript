# The EverTranscript mark

One glyph — a monoline lowercase **e** whose crossbar runs out to the right —
on a petrol-teal tile. The letter that keeps writing: *e* for Ever, the line
for the Transcript. The same drawing is the app icon on every platform, the
website favicon, and (as a monochrome template) the macOS menu bar item.

Everything in `generated/`, plus `clients/electron/resources/` and
`crates/evertranscript-core/src/tray/glyphs/`, is produced by one script:

```sh
pnpm -C brand render
```

The outputs are committed, the script is the source of truth, and a re-run
must produce no diff — it writes a file only when its bytes change, renders
every size from vector, and never rasterizes text (the wordmark is outlined
from the font first), so the results are byte-identical on any machine.

## Palette

| Token | Value | Used for |
|---|---|---|
| `teal-400` | `#158580` | tile gradient, top |
| `teal-500` | `#0F6E6A` | the brand colour; Android icon background; theme colour |
| `teal-700` | `#094F4C` | tile gradient, bottom; dark tile, top |
| `teal-deep` | `#06302E` | dark tile, bottom |
| `paper` | `#F5F1E8` | the glyph on the tile; lockup on dark grounds |
| `ink` | `#1F1D1B` | the glyph and lockup on light grounds |
| `record` | `#E5484D` | the recording accent in UI — never on the icon |

The machine-readable copy of this table is `COLOR` in `render.mjs`.

## Construction

- Masters are drawn on a 256-unit grid, ink within 32…224, as **strokes**:
  one width (32), round caps and joins, `currentColor`. `src/mark.svg` is
  the drawing; `src/mark-small.svg`, when present, replaces it in anything
  rendered at 32 px or less (thicker stroke, less detail).
- The tile is a vertical `teal-400 → teal-700` gradient; the glyph takes
  56 % of the tile's width. No text, no gradient on the glyph, and none of
  the things the product's guarantees forbid the brand to suggest — no
  clouds, no sync arrows, no globes, no padlocks, no sparkles
  (ADR-0001/0020/0034: local-only, inert, provable).
- Consumers that cannot stroke a path (Icon Composer fills whatever it is
  given) get the same drawing as filled outlines, converted by the script —
  `generated/lockups/mark-outlined.svg` is that form on its own.

## What gets generated

| Family | Files |
|---|---|
| macOS | `generated/macos/AppIcon.icns` (16→1024, @1x/@2x); `EverTranscript.icon` — the Icon Composer package for the macOS 26 layered icon, compilable with `actool`, previewable with `ictool` |
| Menu bar | `crates/…/tray/glyphs/{ready,recording,busy,attention}.tiff` — 18 pt templates, 1x+2x in one TIFF, embedded by `tray/macos.rs`; previews in `generated/tray/` |
| Windows | `generated/windows/EverTranscript.ico` (16→256, PNG-compressed) |
| iOS | `generated/ios/Assets.xcassets/AppIcon.appiconset` — opaque 1024 plus iOS 18 `dark` and `tinted` appearances |
| Android | `generated/android/res` — adaptive icon (foreground, colour background, `<monochrome>`), legacy + round mipmaps, `playstore-512.png` |
| Web | `generated/web/` — `favicon.svg`, `favicon.ico`, touch and PWA icons (`maskable-512` keeps the glyph in the 80 % safe zone), `site.webmanifest` |
| Electron | `clients/electron/resources/{icon.icns,icon.ico,icon.png}` — read by `src/main/index.ts` until packaging (M5) bakes them into the bundle |
| Lockups | `generated/lockups/` — the wordmark and mark+wordmark lockups, light and dark, as outlined SVG and preview PNGs |

## The wordmark

"EverTranscript" set in **Geist SemiBold** (`fonts/Geist-SemiBold.ttf`,
OFL — licence beside it) and converted to outlines by the script, so no
consumer ever needs the font. Use the lockups as shipped; don't retype the
name in another face.

## Do / don't

- **Do** put the glyph in `paper` on teal, or in `ink`/`paper` alone on
  plain grounds. **Don't** recolour it, outline it, or set it on another
  colour.
- **Do** use the tray templates for menu bars and the small-size drawing at
  16–32 px. **Don't** shrink the full-size icon below 32 px or use the
  colour icon as a tray image.
- The name and mark identify this project; the files are Apache-2.0 like
  the rest of the repo.

## Two traps this layout avoids

- `.gitignore` has macOS's `Icon?` rule and the repo ignores case, so a
  directory named `icons/` is silently ignored — a `!**/icons/` negation is
  in place, and nothing here uses the name anyway.
- `dist/`, `build/` and `out/` are gitignored, which is why the committed
  outputs live in `generated/`.

## Exploring

The three original candidates and the contact sheet that chose between
them are kept in `explorations/` (`pnpm -C brand render:explorations`
re-renders the sheet). The pick is logged in `DECISIONS.md`.
