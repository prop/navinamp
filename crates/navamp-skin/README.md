# navamp-skin

Wasabi (Winamp3) skin engine — **parsing half** for NavAmp.

Loads `.wal` skin archives and parses their XML into a normalized,
serde-serializable model (`Skin`) that crosses IPC as JSON to the UI, which
renders it with absolute positioning. This crate is deliberately **only a
parser**: no rendering, no MAKI interpretation, no tauri/symphonia/audio
dependencies, and no copyrighted assets (all test fixtures are synthetic and
built in-test).

## Usage

```rust
use std::path::Path;

let archive = navamp_skin::SkinArchive::open(Path::new("skin.wal"))?;
let png = archive.read_file("bitmaps/logo.png")?; // case-insensitive lookup

let (skin, warnings) = navamp_skin::Skin::from_archive(&archive)?;
for warning in &warnings {
    eprintln!("skin warning: {warning}"); // non-fatal, worth logging
}
let json = skin.to_json()?; // pretty JSON for the webview
```

- `SkinArchive::from_bytes(Vec<u8>)` also exists (IPC / in-memory skins).
  The archive is `Send + Sync`; file reads open a fresh ZIP reader per call.
- Parsing never fails on skin *content* problems — those are `Warning`s.
  Only an unreadable archive or a malformed `skin.xml` yields `SkinError`.

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

`cargo test -p navamp-skin` — 13 integration tests (`tests/skin_tests.rs`)
plus a doctest. Fixtures are `.wal` archives written in-test with the `zip`
writer. Coverage: full-model parse (incl. preserved unknown attrs/tags),
case-insensitive lookup, missing bitmap file → warning, dangling group →
warning, group cycles → warnings, includes + include cycle, missing
`skin.xml` → error, malformed numerics → warning + raw preserved, duplicate
groupdef ids → warning, attribute-key casing tolerance, `SkinArchive::open`
from disk, serde JSON round-trip equality.

## Known limitations

- **MAKI (`<script>`) is preserved but not interpreted** — heavily scripted
  skins degrade to their static layout (per plan §3/§10).
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
