# Image-generation prompts (ChatGPT / GPT Image)

Prompts for generating EverTranscript logo imagery with ChatGPT's image
model. They encode the ratified identity (`brand/README.md`,
`DECISIONS.md` Q15–Q18), so outputs land on-brand instead of generic.

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
petrol teal tile `#158580 → #094F4C` (vertical gradient), glyph paper
`#F5F1E8`, ink `#1F1D1B` on light grounds, `#E5484D` reserved for UI
recording accents and banned from the icon.

---

## Prompt 1 — the ratified mark, rendered/polished

Use when you want a faithful, production-feel rendering of the existing
logo (marketing art, richer lighting than the flat SVG).

> The first attached image is the EverTranscript app icon — the target to
> reproduce faithfully; the other images are competitor icons for context
> only, do not copy them. Render a macOS app icon, 1024×1024: a rounded
> square tile with a vertical petrol-teal gradient from #158580 at the top
> to #094F4C at the bottom, carrying one glyph in warm paper white #F5F1E8
> — a geometric monoline lowercase "e" whose horizontal crossbar extends
> past the bowl to the right and ends in a rounded terminal, like a letter
> that keeps writing a line of text. Single uniform stroke weight, round
> caps and joins, the glyph about 56% of the tile's width, optically
> centred. Flat and contemporary with at most a whisper of soft inner
> light; no text, no border, no clouds, no microphones, no sparkles, no
> extra elements. Clean edges, subtle grain-free gradient.

## Prompt 2 — explore variants around the same brief

Use to fish for ideas the vector pipeline wouldn't produce. Anything
worth keeping gets redrawn as a real SVG master in `brand/src/` — never
ship a raster original.

> The attached images are context: the first is EverTranscript's current
> app icon (a monoline lowercase "e" whose crossbar runs out into a line,
> paper white on petrol teal); the rest are competitor icons whose look I
> must NOT copy — one is a hand-drawn spiral on chartreuse, one a heavy
> letterform on cream, one a wordmark on purple. Propose 4 alternative
> app-icon marks for EverTranscript, a local-first meeting notetaker whose
> promise is that recordings never leave the machine. Each mark must be a
> single abstract monoline glyph in paper white #F5F1E8 on a rounded
> square petrol-teal tile (#158580 to #094F4C vertical gradient), one
> stroke weight, round caps, readable at 16 pixels, expressing "speech
> becoming a kept line of text" or "a loop that nothing leaves". Show the
> four as a 2×2 grid on a neutral background, each icon labelled below
> with a number only. Absolutely no text inside the icons, no clouds, no
> microphones, no sound waves with sparkles, no globes, no padlocks.

## Prompt 3 — wordmark / lockup art

The shipping lockup is generated (`brand/generated/lockups/`, Geist
outlined); use this only for exploratory type treatments or hero art.

> Using the first attached image as the exact mark, design a horizontal
> logo lockup for "EverTranscript": the mark at the left, the name set
> once in a clean geometric sans-serif (similar to Geist or Inter
> SemiBold), tight letterspacing, ink colour #1F1D1B on an off-white
> #F5F1E8 background. The mark sits slightly taller than the capital
> letters, vertically centred on them. Generous margins, nothing else in
> frame — no tagline, no decoration, no gradient behind the type.

## Practical notes

- Ask for **1024×1024** (or 1536×1024 for the lockup) and, when you need
  it, "transparent background, PNG" — but remember the iOS 1024 must be
  opaque; the pipeline handles that.
- Generated images are exploration only. The source of truth stays the
  SVG masters + `pnpm -C brand render`; a keeper from Prompt 2 gets
  redrawn on the 256-grid with stroke 32 before it touches the pipeline.
- If the model drifts generic (glossy blobs, gradients on the glyph,
  badge shapes), reattach the current icon and repeat the constraint
  sentence — the negatives are the prompt's real payload.
