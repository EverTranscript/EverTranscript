# Image-generation prompts (ChatGPT / GPT Image)

Prompts for generating EverTranscript logo imagery with ChatGPT's image
model. They encode the ratified identity — the white monoline seahorse on
the coral tile (`brand/README.md`, `DECISIONS.md` Q24–Q26) — so outputs
land on-brand instead of generic.

**Attach with every prompt:**

- `brand/generated/macos/AppIcon-1024.png` — the ratified mark, the anchor
- the three reference logos from `brand/reference/` (untracked; re-extract
  per its README if missing): `granola-appicon-1024.png`,
  `anarlog-appicon.png`, `meetily-appicon.png`

Attached images teach the model far more than adjectives; always tell it
which image is the target and which are only context.

**Ground rules baked into each prompt** — the mark says *speech becomes a
kept record* and may not suggest what the guarantees forbid: no clouds, no
sync arrows, no globes, no padlocks, no microphones, no AI sparkles. One
glyph, one tile, no text inside an app icon (Meetily's mistake). Palette:
coral tile `#FC9E74 → #ED6F62` (vertical gradient; `#B54A3F → #702D26`
for a dark appearance), the glyph pure white, ink `#1F1D1B` and paper
`#F5F1E8` on plain grounds, `#E5484D` reserved for UI recording accents
and banned from the icon.

---

## Prompt 1 — the ratified mark, rendered/polished

Use when you want a faithful, production-feel rendering of the existing
logo (marketing art, richer lighting than the flat SVG).

> The first attached image is the EverTranscript app icon — the target to
> reproduce faithfully; the other images are competitor icons for context
> only, do not copy them. Render a macOS app icon, 1024×1024: a rounded
> square tile with a vertical coral gradient from #FC9E74 at the top to
> #ED6F62 at the bottom, carrying one glyph in pure white — a seahorse
> drawn as a single-weight monoline: a small crown spike topped with a
> round ball, a round dot eye, a duck-bill snout pointing right whose
> mouth is a long straight line running back into the body, a deep
> C-curve of back and belly, and a tail that curls inward into a spiral
> ending in a rounded tip. One uniform stroke weight throughout, round
> caps and joins, the mark standing about three quarters of the tile's
> height, optically centred. Flat and contemporary with at most a whisper
> of soft inner light; no text, no border, no clouds, no microphones, no
> sparkles, no extra elements. Clean edges, subtle grain-free gradient.

## Prompt 2 — explore variations of the seahorse

Use to fish for ideas the vector pipeline wouldn't produce. Anything
worth keeping gets hand-traced into the SVG masters in `brand/src/` —
never ship a raster original.

> The first attached image is EverTranscript's current app icon — a white
> monoline seahorse on a coral gradient tile; the rest are competitor
> icons whose look I must NOT copy — one is a hand-drawn spiral on
> chartreuse, one a heavy letterform on cream, one a wordmark on purple.
> Propose 4 variations of the seahorse mark for EverTranscript, a
> local-first meeting notetaker whose promise is that recordings never
> leave the machine: vary the pose, the tightness of the tail spiral, the
> snout, the amount of detail — but every variation stays a single
> continuous-feeling monoline drawing in pure white on the same rounded
> square coral tile (#FC9E74 to #ED6F62 vertical gradient), one stroke
> weight, round caps, readable at 16 pixels. Show the four as a 2×2 grid
> on a neutral background, each icon labelled below with a number only.
> Absolutely no text inside the icons, no clouds, no microphones, no
> sound waves with sparkles, no globes, no padlocks.

## Prompt 3 — wordmark / lockup art

The shipping lockup is generated (`brand/generated/lockups/`, Geist
outlined); use this only for exploratory type treatments or hero art.

> Using the first attached image as the exact mark, design a horizontal
> logo lockup for "EverTranscript": the seahorse mark at the left drawn
> in ink #1F1D1B, the name set once in a clean geometric sans-serif
> (similar to Geist or Inter SemiBold), tight letterspacing, the same ink
> colour, on an off-white #F5F1E8 background. The mark stands slightly
> taller than the capital letters, vertically centred on them. Generous
> margins, nothing else in frame — no tagline, no decoration, no
> gradient behind the type.

## Practical notes

- Ask for **1024×1024** (or 1536×1024 for the lockup) and, when you need
  it, "transparent background, PNG" — but remember the iOS 1024 must be
  opaque; the pipeline handles that.
- Generated images are exploration only. The source of truth stays the
  SVG masters + `pnpm -C brand render`; a keeper from Prompt 2 gets
  hand-traced onto the 256-grid as stroked paths with a declared ink box
  (the way `src/mark.svg` was made — `DECISIONS.md` Q26) before it
  touches the pipeline.
- If the model drifts generic (glossy blobs, gradients on the glyph,
  badge shapes, extra fins), reattach the current icon and repeat the
  constraint sentence — the negatives are the prompt's real payload.
