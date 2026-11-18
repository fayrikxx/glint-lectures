# Design

Bright, friendly, not minimal for the sake of it. One color family (violet into blue),
very little text on screen, and a small mentor character that explains things in plain words.

## Logo

Three slides stacked close together with soft shadows, and a glowing star on the front one.

| Part | Light | Dark |
| --- | --- | --- |
| Back slide | `#CBBEFF` | `#3E2AA6` |
| Middle slide | `#8E6BFF` | `#5B3DF5` |
| Front slide | `#5B3DF5` | `#7B5CFF` |
| Star | `#FFFFFF` | `#FFFFFF` |
| Star glow | `#BFE3FF` | `#BFE3FF` |
| Icon background | `#F2EFFF` | `#140C3A` |

- Icon shape: rounded square, corner radius 24% of the size.
- Wordmark: **Glint**, Bricolage Grotesque ExtraBold (800), capital G, tracking around -4%.
- Files: `assets/logo.svg`, `assets/logo-dark.svg`.

## Color

| Name | Hex | Use |
| --- | --- | --- |
| Violet | `#6C3BFF` | primary buttons, selection |
| Violet edge | `#3A1FA8` | bottom edge of chunky buttons |
| Blue | `#3D7BFF` | secondary accent, charts |
| Sky | `#7CC4FF` | highlights, text on it `#0D1A40` |
| Lilac | `#C7B3FF` | marker highlight under key phrases |
| Ink | `#1A1240` | text on light |
| Night | `#110A2B` | dark background |
| Sticker | `#FFD84D` | stars and badges only |

Users can switch the accent to Violet `#6C3BFF`, Indigo `#4B3FD9`, Blue `#2F62F0` or Sky `#6CB8FF`.
Body text keeps at least 4.5:1 contrast in every theme.

## Themes

The user picks one on first launch. Layout and features are the same, only the look changes,
so everything is driven by design tokens (CSS variables).

**Bubbly 3D** (default)
- Background `#F4F1FF` / dark `#110A2B`, surfaces `#FFFFFF` / `#1C1442`
- Text `#1A1240` / `#F3F0FF`, muted `#5B5480` / `#B6ACE3`, borders `#DDD3FF` / `#3A2E78`
- Chunky buttons with a solid bottom edge: `box-shadow: 0 5px 0 <edge>`
- Radius 14–28 px
- Fonts: Bricolage Grotesque (headings), DM Sans (text)

**Calm**
- Background `#F7F6FB`, white cards, text `#1A1830`, muted `#5E5B75`
- Primary `#5B3DF5`, soft tint `#EFEBFF`, blue `#4F7BFF`
- Soft shadows: `0 1px 2px rgba(26,24,48,.04), 0 12px 32px rgba(26,24,48,.07)`
- Radius 20–22 px, pill buttons
- Font: Plus Jakarta Sans

**Bold**
- Background `#EDE7FF`, 3 px black borders, hard offset shadows (4–10 px, black)
- Violet `#7B4DFF`, blue `#4E8DFF`, sky `#9FD3FF`, yellow stickers
- No rounding
- Fonts: Archivo Black (headings), Space Grotesk (text)

## The key 20%

- Key phrases on a slide get a marker highlight; the rest of the text is dimmed.
- Numbered markers on the slide match the numbers in the explanation.
- A "Key 20%" panel next to the slide lists the 2–4 ideas that matter.
- The 5-minute view shows a whole lecture as 5 large cards, one idea each, with a link to the source slide.

## Mentor

Working name **Glim**. A puffy four-point star that came out of the logo.

- Body gradient `#FFFFFF` → `#E4F3FF` → `#94C6FF`, glossy highlight top-left
- Round glasses `#5B3DF5`, graduation cap `#1A1240` with a `#FFD84D` tassel
- Pointer `#7B5CFF` with a small gold star at the tip
- Speaks in one or two short sentences, never lectures

Moods: hello, explaining, flying, "got it", thinking, celebrating, "let's try again", paused.
Brightness follows the mood: brightest on "got it", almost off when paused.

Users can change the mentor's color (violet/blue family), accessories (cap, glasses, headphones)
and how it talks (calm, funny, strict) plus how simple the explanations are.

### Sparkles

The mentor leaves small four-point sparkles and dots behind when it moves.

- Light theme: `#FFFFFF` → `#B9D3FF` → `#5866F0` with a thin `#3F4FD0` outline at ~55% so they read on white
- Dark theme: same gradient as the mentor's body
- Bigger sparkles get a thin cross flare and a white center
- Each one twinkles (opacity and scale, ~1.8 s, staggered); no animation with `prefers-reduced-motion`
