# navinamp-skin

Wasabi (Winamp3) skin engine — **parsing + bitmap payloads** for NaVinaMP.

Loads `.wal` skin archives, parses their XML into a normalized,
serde-serializable model (`Skin`) that crosses IPC as JSON to the UI, and
materializes `<bitmap>` elements into PNG data-URLs (`bitmap_payloads`).
Classic Winamp 2 `.wsz` archives are synthesized into the **same** model
(see below), so the rest of the app renders a skin without knowing where it
came from. This crate deliberately does **no rendering, no MAKI
interpretation, no tauri/symphonia/audio**, and ships no copyrighted assets
(all test fixtures are synthetic and built in-test).

## Usage

```rust
use std::path::Path;

let archive = navinamp_skin::SkinArchive::open(Path::new("skin.wal"))?;
let png = archive.read_file("bitmaps/logo.png")?; // case-insensitive lookup

let (skin, warnings) = navinamp_skin::Skin::from_archive(&archive)?;
for warning in &warnings {
    eprintln!("skin warning: {warning}"); // non-fatal, worth logging
}
let json = skin.to_json()?; // pretty JSON for the webview
```

- `SkinArchive::from_bytes(Vec<u8>)` also exists (IPC / in-memory skins).
  The archive is `Send + Sync`; file reads open a fresh ZIP reader per call.
- Parsing never fails on skin *content* problems — those are `Warning`s.
  Only an unreadable archive or a malformed `skin.xml` yields `SkinError`.
- `bitmap_payloads(&archive, &skin)` materializes every `<bitmap>` with a
  `file` into a PNG **data-URL** (`data:image/png;base64,...`): bytes are
  read case-insensitively, decoded as PNG/BMP, cropped to `x`/`y`/`w`/`h`
  when present (absent extents run to the image edge), given Wasabi
  color-key transparency for a `transparent` attribute, and re-encoded.
  Missing/undecodable files, out-of-range crops and unparseable
  `transparent` values are warnings (`MissingFile`, `ImageDecode`,
  `CropOutOfRange`, `MalformedValue`), never fatal; that bitmap just gets
  no payload. Duplicate ids keep the **first** payload. The `transparent`
  color parses liberally: `#RRGGBB`, `RRGGBB`, `#RRGGBBAA` (alpha ignored),
  `0x…` hex, or `R,G,B` / `R,G,B,A` decimals; an exact RGB match becomes
  alpha 0 (classic skins' magic pink `255,0,255`). There is **no
  downscaling on load** — payloads are 1:1 skin pixels and integer scaling
  (1×/2× retina) is the renderer's job. Note `Skin::from_archive` already
  reports missing bitmap files, so calling both yields that warning twice
  unless the caller de-duplicates.

## Format notes

- A `.wal` is a ZIP containing `skin.xml`, optional additional XML files
  pulled in via `<include file="..."/>`, and PNG/BMP/TTF assets. Entry-name
  casing is inconsistent across real skins, so every lookup is
  case-insensitive; original names are preserved in `file_names()`.
- `skin.xml` (or any included file) holds a root `<skin>` element with
  `<elements>` (assets), `<groupdef>`s (reusable object groups) and
  `<container>`s (windows) → `<layout>`s (window modes) → objects.
- **Tag names are matched case-sensitively** (`layer`, not `Layer`), per the
  Wasabi lowercase-first convention. Mixed-case tags found in the wild are
  not a known Winamp3 pattern; if they show up they fall into the
  generic preserve-and-warn path (`Warning::UnknownTag`) rather than being
  dropped — so designing for it means *seeing* it, not guessing.
- **Attribute keys are matched case-insensitively** for typed fields (so
  `File=` still populates `Bitmap::file`), while the original keys and every
  non-typed attribute are stored verbatim in `params`.
- Absent bitmap coordinates (`x`/`y`/`w`/`h`) mean "whole image" and parse
  to `None` (`Bitmap::is_whole_image()`). Numerics are tolerant: malformed
  values parse to `None` + `Warning::MalformedValue`, and the raw string is
  preserved in `params`. Object geometry that fails to be absolute
  (`relatx`/`relaty`/`relatw`/`relath`, `alpha` floats, ...) lives in
  `params` for the renderer to interpret.
- `<group .../>` instances are stored as **references** (group id); the model
  never inlines them. Resolution is the renderer's job, but dangling ids
  (`Warning::MissingGroup`), missing bitmap ids
  (`Warning::MissingBitmapId`) and reference **cycles**
  (`Warning::GroupCycle`) are detected at parse time. Group ids with the
  `wasabi.` prefix are treated as system-library groups and exempt from
  missing-reference warnings.
- Root `<skin>` attributes are preserved on `Skin::params`. `<include>`
  chains are followed (depth ≤ 16); include cycles and malformed *included*
  files are warnings, never fatal.

## Classic Winamp 2 skins (`.wsz`)

A `.wsz` is a ZIP of fixed-layout BMPs plus a few text files — no XML. When
an archive has **no `skin.xml`** and at least **two** of the four hallmark
files (`main.bmp`, `titlebar.bmp`, `cbuttons.bmp`, `pledit.txt`),
`Skin::from_archive` delegates to the synthesizer (`is_classic` /
`synthesize_classic`, re-exported from the crate root) instead of the XML
parser, so `.wal` behavior is byte-for-byte unchanged (a `.wal`-style zip
without `skin.xml` still fails with `SkinError::MissingSkinXml`). Entry
lookups are case-insensitive and tolerate one directory level of nesting
(some `.wsz` zips wrap everything in a folder).

The synthesizer mints `wsz.*`-namespaced bitmap ids and emits real
`<bitmap>` elements, so `bitmap_payloads` crops/transcodes classic BMPs
with the existing pipeline. The full coordinate table lives in the
`wsz.rs` module docs; the coverage matrix:

### Classic format coverage matrix

| Classic file | Handling | Synthesized as |
|---|---|---|
| `main.bmp` | whole-file bitmap `wsz.main` | main container background layer (275×116) |
| `cbuttons.bmp` | 12 crops (6 buttons × normal/pressed rows y=0/18) | `PREV`/`PLAY`/`PAUSE`/`STOP`/`NEXT`/`EJECT` buttons at x=16/39/62/85/108/136, y=88 (eject 89), with `downimage`s |
| `posbar.bmp` | trough (0,0,248×10) + thumbs at x=248/278 | `SEEK` slider at 16,72 |
| `volume.bmp` | frame-0 trough + thumbs at y=422 | `VOLUME` slider at 107,57 (68×13) |
| `balance.bmp` | mid-frame trough (x=9, frame 13) + thumbs | `BALANCE` slider at 177,57 (38×13) |
| `shufrep.bmp` | off/pressed/active cells (rows 0/15/30 and 61/73) | `TOGGLE_SHUFFLE`/`TOGGLE_REPEAT`/`TOGGLE_EQ`/`TOGGLE_PLAYLIST` toggles; active image in `params["activeimage"]` |
| `shuffle.bmp`, `repeat.bmp`, `eq.bmp`, `pl.bmp` | silent fallback whole-file bitmaps | same four toggles when a skin predates `shufrep.bmp` |
| `playpaus.bmp` | 3 cells at x=0/9/18 | one `Status` object at 26,28 (`params` `playimage`/`pauseimage`/`stopimage`) |
| `monoster.bmp` | stereo col x=0 (29px), mono col x=29 (27px), rows on=0/off=12 | `Status` indicators at 212,41 / 239,41 |
| `text.bmp` | `bitmapfont` `wsz.font.text` (5×6 glyphs) | songticker (111,27), bitrate (109,43), kHz (154,43) text objects — glyph rendering is the renderer's job |
| `numbers.bmp` | `bitmapfont` `wsz.font.numbers` (9×13 glyphs) | one `display="time"` text object over the digit region (36..111, y=26) |
| `num0.bmp`…`num9.bmp` | silent whole-file bitmaps `wsz.num.0…9` | wired via `params["numbitmaps"]` on the time object (older dialect) |
| `titlebar.bmp` | glyph grid + strips | `SYSMENU`/`MINIMIZE`/`TOGGLE_SHADE`/`CLOSE` buttons at y=3, clutter-bar layer (10,22); strips feed EQ/playlist titlebars |
| `viscolor.txt` | up to 24 `R,G,B` lines (comments tolerated) | `vis` object at 24,43, `params["viscolors"]` = `r,g,b;…` (semicolon-joined) |
| `pledit.txt` | modern `[Text]`+`Normal=#RRGGBB` **and** ancient bare `R,G,B`-line dialects | `Skin::params`: `pledit.colors` (comma-joined), `pledit.color.{normal,current,normalbg,oldbg,selectedbg,selected}`, `pledit.font` |
| `eqmain.bmp` | bg (0,0), titlebar (0,134/149), ON/AUTO/PRESETS cells (y=119/164/176), graph (0,294), shared trough (13,229) + thumb (0,164/176) | eq container (275×116): `EQ_TOGGLE`/`EQ_AUTO` toggles, `EQ_PRESETS` button, graph layer, 11 vertical `EQ_SET` sliders (preamp index=0 at x=21; bands 1–10 at 78+18i with `params["band"]` = 60…16000 Hz, matching NaVinaMP's EQ) |
| `pledit.bmp` | 3×3 stretchable tile grid | playlist container (275×232) tile layers with `relatx`/`relatw`/… params; the list body is a solid fill from `pledit.color.normalbg` |
| `eq_ex.bmp`, `gen.bmp`, `genex.bmp`, `mb.bmp`, `video.bmp` | not consumed (shade modes / generic plugin frames / minibrowser / video) | documented limitation |
| `REGION.txt` | ignored (window click masks — windows stay rectangular) | documented limitation |

Warning policy: a missing optional file yields exactly **one**
`Warning::ClassicMissingOptional { file, purpose }` and the objects that
would have used it are skipped — a minimal skin with only `main.bmp` +
`cbuttons.bmp` still loads, minus everything else. `main.bmp` itself is
tolerated as missing too. Unparseable `pledit.txt` color values warn per
line (`MalformedValue`, element `pledit.txt`) and are dropped. Only an
unreadable ZIP is fatal.

## Coverage matrix

### `<elements>` children

| Tag | Handling | Notes |
|---|---|---|
| `bitmap` | typed — `Elements::bitmaps` | `id`, `file`, `x`/`y`/`w`/`h` (`u32`, absent = whole image); extras (e.g. `gammagroup`) → `params` |
| `bitmapfont` | typed — `Elements::bitmap_fonts` | `id`, `file`, `charwidth`, `charheight`; extras → `params` |
| `truetypefont` | typed — `Elements::truetype_fonts` | `id`, `file`; extras → `params` |
| `color` | typed — `Elements::colors` | `id`, `value` kept raw (skins use several notations); extras → `params` |
| `gamma` | typed — `Elements::gammas` | attrs → `params`, child tags preserved generically |
| anything else | generic — `Elements::other` | full subtree kept (`GenericNode`) + `Warning::UnknownTag` |

### Objects (inside `groupdef` / `layout`)

| Tag | Handling | Typed fields (rest → `params`) |
|---|---|---|
| `layer` | typed — `GuiObject::Layer` | `id`, `image`, `x`/`y`/`w`/`h` (`i32`) |
| `animatedlayer` | typed — `GuiObject::AnimatedLayer` | `id`, `image`, rect |
| `button` | typed — `GuiObject::Button` | `id`, `action`, `image`, `downimage`, `hoverimage`, `tooltip`, rect (e.g. `cfg`, `ghost`, `notify` → `params`) |
| `togglebutton` | typed — `GuiObject::ToggleButton` | same shape as button |
| `slider` | typed — `GuiObject::Slider` | `id`, `action` (+ `action_kind()` → `Seek`/`Volume`/`Balance`), `image`, `downimage`, `hoverimage`, `thumbimage`, rect |
| `text` | typed — `GuiObject::Text` | `id`, `display` (e.g. `songticker`), rect |
| `vis` | typed — `GuiObject::Vis` | `id`, rect |
| `status` | typed — `GuiObject::Status` | `id`, rect |
| `grid` | typed — `GuiObject::Grid` | `id`, `image`, rect |
| `group` | typed — `GuiObject::Group` | instance **reference**: `id`, rect; rare inner content kept as generic children |
| `groupdef` (nested) | hoisted into `Skin::groups` | liberal: parsed as a definition wherever it appears |
| anything else | generic — `GuiObject::Other` | full subtree kept + `Warning::UnknownTag` (renderer skips gracefully) |

### Structure

| Tag | Handling |
|---|---|
| `skin` (root) | attrs → `Skin::params` |
| `container` | typed — `Skin::containers`: `id`, `name`, `params`, `layouts`, generic `other` |
| `layout` | typed — `Container::layouts`: `id`, rect, `params` (`background`, `autoatstart`, shade-mode flags...), `objects` |
| `elements` | typed — `Skin::elements` |
| `include` | followed (case-insensitive file lookup); cycle/missing → warning |
| `script`/`scripts`/MAKI, `accelerators`, unknown top-level tags | generic — `Skin::other` + `Warning::UnknownTag` (MAKI is out of scope by design) |

Every model type derives `Debug, Clone, PartialEq, Eq, Serialize, Deserialize`
(no floats in typed fields), so `Skin → JSON → Skin` is losslessly
round-trippable, and `Skin::to_json()` is provided for IPC.

## Testing

`cargo test -p navinamp-skin` — 13 parse-half integration tests
(`tests/skin_tests.rs`) + 9 bitmap-payload integration tests
(`tests/image_tests.rs`) + 13 classic-`.wsz` synthesis tests
(`tests/wsz_tests.rs`, fixtures are BMPs encoded in-test with the `image`
crate at the true base-skin dimensions) + the fallback-skin checks
(`tests/fallback_skin.rs`) + unit tests for the color-key parser and the
classic text-file parsers, plus doctests.
Fixtures are `.wal`/`.wsz` archives written in-test with the `zip` writer.
Coverage: full-model parse (incl. preserved unknown attrs/tags),
case-insensitive lookup, missing bitmap file → warning, dangling group →
warning, group cycles → warnings, includes + include cycle, missing
`skin.xml` → error, malformed numerics → warning + raw preserved,
duplicate groupdef ids → warning, attribute-key casing tolerance,
`SkinArchive::open` from disk, serde JSON round-trip equality;
classic detection (vs `.wal` vs junk zips), the three synthesized
containers and their layout rects, transport/toggle/slider actions and
image wiring, EQ band indices/frequencies, missing-optionals → one warning
per file with objects skipped, both `pledit.txt` dialects (incl. junk
header), the 24-entry viscolor blob, `.wal` regression (XML path
untouched), and every synthesized crop verified to fit fixture BMPs of
real classic dimensions through `bitmap_payloads`;
whole-image payloads sample-verified against source pixels, crop
correctness (incl. x-only crops and case-differing file refs), magic-pink
BMP → alpha transparency (all `transparent` notations), unparseable
`transparent` → no-op warning, missing/undecodable files and out-of-range
crops → warnings without payloads, duplicate bitmap ids keep the first
payload, bitmaps without `file` are skipped silently.

## Known limitations

- **Classic `.wsz` skins**: `REGION.txt` click masks are ignored (windows
  stay rectangular); windowshade/shade layouts, `eq_ex.bmp`, `gen.bmp`/
  `genex.bmp` generic frames, the minibrowser and video windows are not
  synthesized; `text.bmp`/`numbers.bmp` glyph *rendering* is the
  renderer's job (the model wires fonts and digit bitmaps to the text
  objects); playlist list rows are renderer-drawn from the
  `pledit.color.*` params. `pledit.txt`'s ancient bare-line dialect maps
  its 4th color to `oldbg` (it has no modern key).
- **MAKI (`<script>`) is preserved but not interpreted** — heavily scripted
  skins degrade to their static layout (per plan §3/§10).
- Bitmap payloads are PNG/BMP-decode only (the `image` crate is built with
  exactly those features); other formats warn and skip. Color-key
  transparency is an exact RGB match — anti-aliased edges around magic
  pink are not cleaned up, and no downscaling happens on load (the
  renderer owns integer scaling).
- Tag-name matching is case-sensitive: a hypothetical `<Bitmap>` is kept as
  a generic node with a warning, not parsed as a bitmap (see format notes).
- Non-UTF-8 XML: quick-xml 0.42 stores attribute values as UTF-8; skins in
  exotic legacy codepages may need the quick-xml `encoding` feature added
  here later (one-line change).
- `relatx`/`relaty`-style relative geometry, `alpha`, and `autoplayseq`-
  style animated-layer parameters are preserved in `params` but have no
  typed fields; interpreting them is the renderer's job.
- Text content of *typed* object tags (e.g. `<text>fallback</text>`) is not
  captured — only unknown/generic nodes keep their text. Not seen in real
  Winamp3 skins.
- Only `deflate`/`bzip2` ZIP methods are enabled; WAL archives in the wild
  are deflate.
