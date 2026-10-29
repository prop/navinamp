# NavAmp Fallback Skin

The built-in skin for NavAmp, in the spirit of the Winamp3 default: a
275×280 player window (plus a 275×14 shade strip), a 275×232 playlist
editor, dark metallic chrome, brushed-steel body, recessed near-black
display panels with green (#00CC33) readouts, 1px bevels everywhere, and a
5×7 bitmap font.

**All artwork is original and drawn programmatically** by
[`build.mjs`](./build.mjs) — no Nullsoft assets, no downloaded skins
(MIT-licensed recreation of the *look*, per plan §3/§10).

## Build

```sh
cd assets/fallback-skin
npm install        # dev deps only: pngjs + fflate
node build.mjs     # → out/fallback.wal  (74 KB, ZIP: skin.xml + 33 PNGs)
```

Node ≥ 20, no network at build time. The script:

1. paints every PNG from a fixed palette with integer-hash noise
   (no `Math.random`, no `Date.now`),
2. emits `skin.xml` from the same declarative bitmap table,
3. packs the `.wal` with sorted lowercase entries and **fixed ZIP
   timestamps** (2026-01-01), so the output is byte-identical on every run
   (the script builds the whole skin twice and fails if the bytes differ;
   sha256 is printed),
4. **verifies itself**: re-opens the zip, requires every `file=` referenced
   in `skin.xml` to exist in the archive (and no orphan PNGs), unique
   bitmap/bitmapfont/color/groupdef/container ids, every
   `image`/`downimage`/`hoverimage`/`thumbimage`/`background` to resolve to
   a `<bitmap>` id, every `<group id=…>` to have a matching `<groupdef>`,
   well-formed integer `x`/`y`/`w`/`h`/`charwidth`/`charheight`, and only
   known lowercase Wasabi tags — anything else fails the build. These are
   exactly the conditions `crates/navamp-skin` reports as warnings, so a
   clean build here parses with zero warnings there.

## Windows & metrics

### Player — container `main`, layout `normal`, 275×280

| y-range | Content |
|---|---|
| 0–13 | Gradient title bar (group `player.titlebar`): embossed **NAVAMP** logo, decorative min/shade/close togglebuttons (14×10) at x=227/243/259 |
| 16–75 | Recessed display panel (group `player.display`, 255×60 at 10,16): songticker text, 4-frame playstate `animatedlayer` (16×16), big time readout — dim ghost "88:88" bitmap + `text display="time"` overlay, `text display="status"`, kbps/khz ghosts, MONO/STEREO indicator |
| 80–92 | Seek slider (`action="SEEK"`, 255×13, ticks every 12px) |
| 96–108 | Balance (x=10, 110×13, center detent tick) + volume (x=155, 110×13) sliders |
| 114–135 | Transport group `player.transport` at 52,114: PREV PLAY PAUSE STOP NEXT EJECT, 26×22 each, gap 3 |
| 141–236 | Recessed vis panel 255×96 with dot grid + `vis` object at 14,145 (247×88) |
| 243–260 | Bottom strip: PLAYLIST + SHADE togglebuttons (54×18) at x=10/68 |
| 261–279 | Footer groove + etched NAVAMP logo at 120,266 |

The titlebar's shade window-button carries `action="TOGGLE_SHADE"` (the
playlist's copy stays decorative); min/close have no standard action yet.

### Player — container `main`, layout `shade`, 275×14

A window-shade strip (group `player.shadebar` over the same titlebar
bitmap): embossed **NAVAMP** mini logo at x=3, live songticker
(`display="songticker"`, 118px) at x=44, live time (`display="time"`) at
x=164, then play / pause / stop / next mini buttons (13×12, 7px glyphs)
and a restore toggle (`action="TOGGLE_SHADE"`, boxed-arrow glyph) ending
at x=273. Everything left of the button cluster (x=0..203) is the OS drag
region; double-clicking it toggles back to normal. The app detects shade
strips generically as "layouts with no `player.titlebar` group that are
≤16px tall" and overlays the drag region accordingly.

### Playlist — container `playlist`, layout `normal`, 275×232

| y-range | Content |
|---|---|
| 0–13 | Title bar (group `playlist.titlebar`), embossed **PLAYLIST** logo, same window buttons |
| 17–190 | Recessed list panel 265×174 at 5,17; row height **13**, rows at x=9 w=257. Positionable row bitmaps: `playlist.row.selected` (blue-steel gradient), `playlist.row.current` (green underline + faint wash, alpha PNG), `playlist.row.divider` (1px bottom line, alpha PNG) |
| 201–217 | Action group `playlist.actions` at 8,201: ADD REM SEL MISC (36×16 each, gap 3) |
| 203–215 | Totals: dim ghost "0 ITEMS 0:00" + `status` object |
| 223–230 | Resize grip (diagonal grooves) at 258,223 |

## Color system

One steel ramp + ink + the display greens (also declared as `<color>`
elements, ids `player.green*`, `player.vis.*`, `playlist.text*`):

| Role | Hex |
|---|---|
| Ink (1px outlines) | `#101317` |
| Bevel dark / gradient bottom | `#2C3138` |
| Steel body gradient | `#6A7179` → `#4D535A` (±3 brushed streaks) |
| Bevel light | `#C3CAD2` |
| Display fill | `#060A08` → `#0B110D` |
| **Green** (primary display) | `#00CC33` |
| Green dim (ghosts, labels) | `#00801F` |
| Green bright (peaks, active) | `#66FF99` |
| Selected row | `#4E6D8A` → `#2B4359` (blue-steel) |

Bevel convention: raised = light top/left + dark bottom/right + ink
outline; recessed (display/vis/list/channels) = inverted. Gradients are
float-lerped per row (no banding); noise is a deterministic integer hash
(`hash2`), so every pixel is reproducible.

## Pixel font

A 5×7 font (digits, A–Z, ASCII 32–95 punctuation, lowercase maps to
uppercase) is defined as glyph maps in `build.mjs` and used three ways:

- `player-fonts/font.png` — a 384×8 strip (64 cells of 6×8), declared as
  `<bitmapfont id="player.font" charwidth="6" charheight="8"/>`,
- static labels rendered **at 1×** from the same maps: title logos
  (embossed), kbps/khz/mono-stereo, playlist totals, button labels,
- the time ghost rendered **at 2×** (nearest-neighbor, crisp) — the live
  time (`text display="time"`) paints bright digits over the dim `88:88`.

## Bitmaps

87 `<bitmap>` definitions over 33 PNG files. Whole-file bitmaps for
window/panel/channel art; five **sprite sheets** with `x`/`y`/`w`/`h`
sub-rects where a grid is natural:

| Sheet | Cells | Bitmap ids |
|---|---|---|
| `player/transport.png` (156×66) | 6 glyphs × 3 states, 26×22 | `player.btn.{prev,play,pause,stop,next,eject}.{normal,hover,down}` |
| `player/titlebar-buttons.png` (42×30) | 3 × 3, 14×10 | `player.win.{min,shade,close}.*` (shared by both title bars) |
| `player/toggles.png` (108×54) | 2 × 3, 54×18 | `player.toggle.{playlist,shade}.*` |
| `player/shade-buttons.png` (65×36) | 5 × 3, 13×12 | `player.shadebtn.{play,pause,stop,next,restore}.*` |
| `playlist/actions.png` (144×48) | 4 × 3, 36×16 | `playlist.btn.{add,rem,sel,misc}.*` |

Button states: normal (raised, dark engraved glyph + light emboss), hover
(+8 brightness), down (sunken, glyph nudged 1px; toggle/action buttons glow
green). Transport glyphs (⏮ ▶ ⏸ ⏹ ⏭ ⏏) are generated pixel triangles/bars,
not font characters.

## Renderer notes / intentional gaps

- The layout also carries `background="…"` (authentic Wasabi), but the
  window base is a plain `<layer>` at (0,0) because that is what the
  resolver paints today.
- kbps/khz/mono-stereo are **static ghost bitmaps** — the renderer only
  implements `display="songticker" | "time" | "status"`; live bitrate
  fields would need new display kinds. Same for playlist row text.
- `text display="time"` carries `text=" "` so the current CSS-text
  renderer paints nothing over the ghost bitmap; when live time lands,
  `display` should take precedence over `params.text`.
- Slider thumbs are 13×13 squares (the renderer sizes thumbs to the track
  cross-dimension). `vis`/`status` currently render as labelled
  placeholders; the recessed panels beneath them are final art.
