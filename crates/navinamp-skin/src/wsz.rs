//! Classic Winamp 2 (`.wsz`) skin synthesis: fixed-layout BMPs in, the same
//! normalized [`Skin`] model the `.wal` parser produces, out.
//!
//! A `.wsz` is a ZIP of BMPs plus a few text files — no XML anywhere. The
//! classic layout is fully standardized (window sizes, control positions and
//! the sub-rectangles inside each strip BMP), so this module *synthesizes*
//! the containers/objects a Winamp3 `skin.xml` would have declared, while
//! emitting every consumed BMP as a proper `<bitmap id="wsz.…" file="…">`
//! element. The rest of the app (payload pipeline, renderer) then treats a
//! classic skin exactly like a Wasabi one — which is the entire point of the
//! normalized model.
//!
//! Every bitmap id minted here is namespaced `wsz.*`, so it can never collide
//! with ids from a real `skin.xml`. Missing optional files (minimal classic
//! skins routinely ship without `eqmain.bmp` etc.) skip the objects that
//! would have used them and cost a [`Warning::ClassicMissingOptional`] — one
//! warning per missing file — including `main.bmp` itself (the main window
//! still exists, minus its background). The only fatal case is an unreadable
//! ZIP, which [`SkinArchive::from_bytes`](crate::SkinArchive::from_bytes)
//! already rejects before synthesis starts.
//!
//! # Coordinate table
//!
//! The constants below are the classic-spec layout, cross-checked against the
//! Winamp 2.91 base skin's actual BMP dimensions and two independent
//! open-source implementations (Webamp's CSS positions and Webamp-modern's
//! Wasabi XML). Where sources disagree the widely-cited value is used and the
//! deviation noted at the use site. All coordinates are window pixels
//! (player and EQ windows are 275×116, the playlist is 275×232).
//!
//! | Object | Window rect | Source rect (file) |
//! |---|---|---|
//! | player background | 0,0 275×116 | `main.bmp` whole |
//! | prev/play/pause/stop | 16/39/62/85,88 23×16 | `cbuttons.bmp` x=0/23/46/69, row y=0, pressed row y=18 |
//! | next | 108,88 22×16 | `cbuttons.bmp` x=92 |
//! | eject | 136,89 22×16 | `cbuttons.bmp` x=114 |
//! | seek slider | 16,72 248×10 | `posbar.bmp` trough 0,0; thumb x=248; pressed x=278 |
//! | volume slider | 107,57 68×13 | `volume.bmp` frame 0 (0,0); thumbs y=422 |
//! | balance slider | 177,57 38×13 | `balance.bmp` frames x=9; thumbs y=422 |
//! | shuffle toggle | 164,89 46×15 | `shufrep.bmp` x=28, rows y=0/15/30 |
//! | repeat toggle | 210,89 28×15 | `shufrep.bmp` x=0, rows y=0/15/30 |
//! | EQ toggle | 219,58 23×12 | `shufrep.bmp` (0,61)/(46,73)/(0,73) |
//! | playlist toggle | 242,58 23×12 | `shufrep.bmp` (23,61)/(69,73)/(23,73) |
//! | songticker | 111,27 155×9 | font `text.bmp` 5×6 glyphs (`params` `font`/`charset`) |
//! | time display | 36,26 75×13 | font `numbers.bmp` 9×13 glyphs; digits at `params["digitoffsets"]` |
//! | play/pause/stop status | 26,28 9×9 | `playpaus.bmp` x=0/9/18 |
//! | mono/stereo indicators | 212/239,41 27/29×12 | `monoster.bmp` x=29/x=0, rows y=0 (on) / y=12 (off) |
//! | visualizer | 24,43 72×16 | colors from `viscolor.txt` |
//! | sysmenu/minimize/shade/close | 6/244/254/264,3 9×9 | `titlebar.bmp` glyph grid |
//! | clutter bar | 10,22 8×43 | `titlebar.bmp` x=304 |
//! | bitrate / kHz digits | 111/156,43 15/10×9 | font `text.bmp`; the `kbps`/`khz` labels are baked into `main.bmp` |
//! | EQ background | 0,0 275×116 | `eqmain.bmp` (0,0) |
//! | EQ titlebar | 0,0 275×14 | `eqmain.bmp` (0,134) active / (0,149) inactive |
//! | EQ on/auto/presets | 14/39/217,18 | `eqmain.bmp` (10/35/224,119…) |
//! | EQ curve graph | 86,17 113×19 | `eqmain.bmp` (0,294) |
//! | EQ preamp slider | 21,38 14×63 | track `eqmain.bmp` (13,229); thumb (0,164)/(0,176) |
//! | EQ band slider i (0–9) | 78+18·i,38 14×63 | track `eqmain.bmp` (13,229); thumb shared |
//! | playlist frame | 0,0 275×232 | `pledit.bmp`: 20px titlebar, 12/20px side rails, 38px bottom bar (`tile=x/y`); markers `pl.list`/`pl.totals`/`playlist.titlebar.zone`; hotspots over the baked buttons |
//!
//! `REGION.txt` (window click masks) is deliberately ignored — windows stay
//! rectangular; see the README's limitations. Glyph *rendering* (`text.bmp`,
//! `numbers.bmp`) is the renderer's job (src/lib/skin/bmfont.ts); this module
//! wires the fonts to the text objects.
//!
//! # Text-object conventions
//!
//! Classic text objects carry their semantics in `params` (the Wasabi
//! `display` attribute stays reserved for `songticker`/`time`/`status`):
//!
//! | `params` key | Values | Meaning |
//! |---|---|---|
//! | `font` | `wsz.font.text` / `wsz.font.numbers` | `<bitmapfont>` id whose glyph strip paints the object |
//! | `charset` | `classic-text` / `classic-numbers` | cell map of that strip (see bmfont.ts) |
//! | `digitoffsets` | e.g. `12,24,42,54` | x offsets of the time display's four digit cells inside the object rect (the `:` colon is baked into main.bmp, so there is no colon glyph to draw) |
//!
//! Readout semantics: the bitrate/kHz **Text** objects use the typed `display`
//! attribute with the values `bitrate`/`khz` (filled from the engine's
//! `audio_state` metadata — the `kbps`/`khz` labels beside them are **baked
//! into** main.bmp); the mono/stereo **Status** objects use `params["display"]`
//! with the values `mono`/`stereo` (the channel-indicator labels: dim
//! `image` until the engine reports channels, lit `activeimage` after).
//! (`params` is serde-flattened, so a Text object cannot carry a
//! `params["display"]` distinct from its typed attribute — the Status
//! objects have no typed display field, which is why their convention rides
//! in params.)
//!
//! The glyph strips themselves are emitted a second time as whole-file
//! `<bitmap>` elements (`wsz.font.text.image`, `wsz.font.numbers.image`) so
//! `bitmap_payloads` materializes them — the renderer looks the strip up as
//! `images["{font}.image"]`.

use std::collections::BTreeSet;

use crate::archive::SkinArchive;
use crate::error::{SkinError, Warning};
use crate::model::*;

/// Hallmark files of the classic format: detection requires **no** skin.xml
/// (the Wasabi hallmark) plus at least two of these. Every real `.wsz` has
/// `main.bmp` and almost all also one of `titlebar.bmp` / `cbuttons.bmp` /
/// `pledit.txt`; a two-of-four rule tolerates minimal skins while refusing
/// arbitrary ZIPs.
const HALLMARKS: [&str; 4] = ["main.bmp", "titlebar.bmp", "cbuttons.bmp", "pledit.txt"];

/// Classic player/EQ window size (the playlist is taller; see
/// [`PLAYLIST_H`]).
const WINDOW_W: i32 = 275;
const WINDOW_H: i32 = 116;
/// Classic default playlist height.
const PLAYLIST_H: i32 = 232;

/// EQ band centre frequencies, labelled left→right on the classic skin —
/// identical to NaVinaMP's own EQ (60 Hz … 16 kHz).
const EQ_BANDS_HZ: [u32; 10] = [60, 170, 310, 600, 1000, 3000, 6000, 12000, 14000, 16000];

/// True when the archive looks like a classic Winamp 2 skin: no `skin.xml`
/// and at least two hallmark files present. Lookups are case-insensitive and
/// tolerate one directory level of nesting (some `.wsz` zips wrap their
/// files in a folder).
pub fn is_classic(archive: &SkinArchive) -> bool {
    if archive.contains(crate::archive::SKIN_XML) {
        return false;
    }
    HALLMARKS
        .iter()
        .filter(|file| find_file(archive, file).is_some())
        .count()
        >= 2
}

/// Synthesizes the normalized [`Skin`] model from a classic archive.
///
/// Never fails on content: missing optional BMPs skip their objects with a
/// [`Warning::ClassicMissingOptional`] and unparseable `pledit.txt` color
/// lines warn and drop just that line.
pub fn synthesize(archive: &SkinArchive) -> Result<(Skin, Vec<Warning>), SkinError> {
    let mut out = Builder {
        archive,
        skin: Skin::default(),
        warnings: Vec::new(),
        warned_missing: BTreeSet::new(),
    };
    out.skin
        .params
        .insert("format".to_owned(), "classic-wsz".to_owned());

    let viscolors = VisColors::parse(&read_text(archive, "viscolor.txt"));
    let pledit = PleditColors::parse(&read_text(archive, "pledit.txt"), &mut out.warnings);
    out.apply_pledit_params(&pledit);

    main_window(&mut out, &viscolors);
    eq_window(&mut out);
    playlist_window(&mut out);

    // References are consistent by construction (a bitmap is emitted iff its
    // file exists, and objects referencing it are emitted iff the bitmap is),
    // but validation is cheap and keeps future edits honest.
    crate::validate::validate(&out.skin, archive, &mut out.warnings);
    let mut warnings = out.warnings;
    dedupe(&mut warnings);
    Ok((out.skin, warnings))
}

// ---- file lookup -----------------------------------------------------------

/// Finds `name` in the archive, case-insensitively, first at the root and
/// then in any single subdirectory (some `.wsz` zips nest everything one
/// folder deep). Returns the *actual* entry name so emitted `file=`
/// attributes point at the real entry.
fn find_file(archive: &SkinArchive, name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let suffix = format!("/{lower}");
    let mut nested: Option<String> = None;
    for entry in archive.file_names() {
        let entry_lower = entry.to_ascii_lowercase();
        if entry_lower == lower {
            return Some(entry); // root-level match wins outright
        }
        if entry_lower.ends_with(&suffix) && nested.is_none() {
            nested = Some(entry);
        }
    }
    nested
}

/// Decoded `(width, height)` of an archive image (BMP/PNG). `None` means the
/// entry was unreadable or not a format the `image` crate handles.
fn image_size(archive: &SkinArchive, file: &str) -> Option<(u32, u32)> {
    let bytes = archive.read_file(file).ok()?;
    let decoded = image::load_from_memory(&bytes).ok()?;
    Some((decoded.width(), decoded.height()))
}

/// Reads a text entry if present; missing text files are fine (they are
/// optional in the classic spec) and yield an empty string.
fn read_text(archive: &SkinArchive, name: &str) -> String {
    match find_file(archive, name) {
        Some(actual) => archive
            .read_file(&actual)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default(),
        None => String::new(),
    }
}

// ---- builder ----------------------------------------------------------------

/// Accumulates the synthesized model and its warnings.
struct Builder<'a> {
    archive: &'a SkinArchive,
    skin: Skin,
    warnings: Vec<Warning>,
    /// Lowercased file names already warned about as missing (one warning
    /// per file, not per would-be consumer).
    warned_missing: BTreeSet<String>,
}

impl Builder<'_> {
    /// Emits a cropped bitmap element (id `wsz.{id}`) if `file` exists,
    /// returning the full bitmap id. Missing file → warning + `None`.
    fn bitmap(&mut self, id: &str, file: &str, x: u32, y: u32, w: u32, h: u32) -> Option<String> {
        let Some(actual) = find_file(self.archive, file) else {
            self.missing(file, id);
            return None;
        };
        let full_id = format!("wsz.{id}");
        self.skin.elements.bitmaps.push(Bitmap {
            id: full_id.clone(),
            file: Some(actual),
            x: Some(x),
            y: Some(y),
            w: Some(w),
            h: Some(h),
            params: Params::new(),
        });
        Some(full_id)
    }

    /// Emits a whole-file bitmap element (no crop rect).
    fn whole_bitmap(&mut self, id: &str, file: &str) -> Option<String> {
        let Some(actual) = find_file(self.archive, file) else {
            self.missing(file, id);
            return None;
        };
        self.push_whole(id, &actual);
        Some(format!("wsz.{id}"))
    }

    /// Like [`Builder::whole_bitmap`] but silent when absent — for
    /// alternative-format files (num0.bmp sets, separate shuffle.bmp-era
    /// controls) whose absence is normal, not worth a warning.
    fn present_bitmap(&mut self, id: &str, file: &str) -> Option<String> {
        find_file(self.archive, file).map(|actual| {
            self.push_whole(id, &actual);
            format!("wsz.{id}")
        })
    }

    fn push_whole(&mut self, id: &str, actual: &str) {
        self.skin.elements.bitmaps.push(Bitmap {
            id: format!("wsz.{id}"),
            file: Some(actual.to_owned()),
            params: Params::new(),
            ..Bitmap::default()
        });
    }

    /// Emits a `<bitmapfont>` for a glyph strip (`text.bmp`, `numbers.bmp`).
    fn bitmap_font(&mut self, id: &str, file: &str, w: u32, h: u32) -> Option<String> {
        let Some(actual) = find_file(self.archive, file) else {
            self.missing(file, id);
            return None;
        };
        let full_id = format!("wsz.{id}");
        self.skin.elements.bitmap_fonts.push(BitmapFont {
            id: full_id.clone(),
            file: Some(actual),
            charwidth: Some(w),
            charheight: Some(h),
            params: Params::new(),
        });
        Some(full_id)
    }

    fn missing(&mut self, file: &str, purpose: &str) {
        if self.warned_missing.insert(file.to_ascii_lowercase()) {
            self.warnings.push(Warning::ClassicMissingOptional {
                file: file.to_owned(),
                purpose: purpose.to_owned(),
            });
        }
    }

    fn container(&mut self, id: &str, name: &str) {
        self.skin.containers.push(Container {
            id: id.to_owned(),
            name: Some(name.to_owned()),
            params: Params::new(),
            layouts: Vec::new(),
            other: Vec::new(),
        });
    }

    /// Appends `objects` as the `id` layout of the last container.
    fn finish_layout(&mut self, id: &str, w: i32, h: i32, objects: Vec<GuiObject>) {
        let container = self.skin.containers.last_mut().expect("container pushed");
        container.layouts.push(Layout {
            id: id.to_owned(),
            rect: rect(0, 0, w, h),
            params: Params::new(),
            objects,
        });
    }

    /// Flattens parsed pledit.txt colors into skin-level params:
    /// `pledit.colors` (comma-joined `key=#rrggbb` in parse order) plus one
    /// `pledit.color.{key}` per parsed color and `pledit.font`, so the HTML
    /// playlist can adopt classic colors later.
    fn apply_pledit_params(&mut self, pledit: &PleditColors) {
        if pledit.colors.is_empty() && pledit.font.is_none() {
            return;
        }
        let joined = pledit
            .colors
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(",");
        self.skin.params.insert("pledit.colors".to_owned(), joined);
        for (key, value) in &pledit.colors {
            self.skin
                .params
                .insert(format!("pledit.color.{key}"), value.clone());
        }
        if let Some(font) = &pledit.font {
            self.skin
                .params
                .insert("pledit.font".to_owned(), font.clone());
        }
    }
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect {
        x: Some(x),
        y: Some(y),
        w: Some(w),
        h: Some(h),
    }
}

/// Window placement (position + size) for the object constructors below —
/// keeps their signatures inside clippy's argument budget.
#[derive(Debug, Clone, Copy)]
struct Place {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

const fn place(x: i32, y: i32, w: i32, h: i32) -> Place {
    Place { x, y, w, h }
}

impl Place {
    fn rect(self) -> Rect {
        rect(self.x, self.y, self.w, self.h)
    }
}

/// Removes exact duplicate warnings while keeping first-occurrence order
/// (mirrors the XML parser's post-processing).
fn dedupe(warnings: &mut Vec<Warning>) {
    let mut seen: Vec<Warning> = Vec::new();
    warnings.retain(|w| {
        if seen.contains(w) {
            false
        } else {
            seen.push(w.clone());
            true
        }
    });
}

// ---- object constructors ----------------------------------------------------

fn layer(id: &str, image: String, x: i32, y: i32, w: i32, h: i32) -> GuiObject {
    GuiObject::Layer(Layer {
        id: Some(format!("wsz.{id}")),
        image: Some(image),
        rect: rect(x, y, w, h),
        params: Params::new(),
    })
}

fn button(
    id: &str,
    action: &str,
    image: String,
    down: Option<String>,
    at: Place,
    tooltip: &str,
) -> GuiObject {
    GuiObject::Button(Button {
        id: Some(format!("wsz.{id}")),
        action: Some(action.to_owned()),
        image: Some(image),
        downimage: down,
        hoverimage: None,
        tooltip: Some(tooltip.to_owned()),
        rect: at.rect(),
        params: Params::new(),
    })
}

/// A toggle with the classic three-state images (off / pressed / active).
/// The model has no typed `activeimage`, so it rides in `params` — exactly
/// where a Wasabi parser would preserve an unknown attribute.
fn toggle(
    id: &str,
    action: &str,
    image: String,
    down: Option<String>,
    active: Option<String>,
    at: Place,
    tooltip: &str,
) -> GuiObject {
    let mut params = Params::new();
    if let Some(active) = active {
        params.insert("activeimage".to_owned(), active);
    }
    GuiObject::ToggleButton(ToggleButton {
        id: Some(format!("wsz.{id}")),
        action: Some(action.to_owned()),
        image: Some(image),
        downimage: down,
        hoverimage: None,
        tooltip: Some(tooltip.to_owned()),
        rect: at.rect(),
        params,
    })
}

/// Image-less click target. Classic skins bake the glyphs for the playlist
/// titlebar/action bar into `pledit.bmp`; the hotspot only supplies the hit
/// area and a semantic action, so `SkinButton` renders it transparent.
fn hotspot(id: &str, action: &str, at: Place, tooltip: &str) -> GuiObject {
    GuiObject::Button(Button {
        id: Some(format!("wsz.{id}")),
        action: Some(action.to_owned()),
        image: None,
        downimage: None,
        hoverimage: None,
        tooltip: Some(tooltip.to_owned()),
        rect: at.rect(),
        params: Params::new(),
    })
}

/// Invisible geometry marker (no bitmap): the renderer reads the rect to
/// place HTML content over the skin (playlist list body, totals readout,
/// titlebar drag band).
fn marker(id: &str, at: Place) -> GuiObject {
    GuiObject::Layer(Layer {
        id: Some(format!("wsz.{id}")),
        image: None,
        rect: at.rect(),
        params: Params::new(),
    })
}

/// A bitmap plane repeated along one axis (`tile=x|y|1`). Classic pledit
/// frames are stretchable 3×3 grids: the center strip tiles horizontally and
/// the side rails tile vertically.
fn tiled_layer(id: &str, image: String, at: Place, axis: &str, mut params: Params) -> GuiObject {
    params.insert("tile".to_owned(), axis.to_owned());
    GuiObject::Layer(Layer {
        id: Some(format!("wsz.{id}")),
        image: Some(image),
        rect: at.rect(),
        params,
    })
}

/// Slider with the classic thumb trio: `thumbimage` typed, the pressed thumb
/// in `params["thumbdownimage"]` (the model has no typed field for it).
fn slider(
    id: &str,
    action: &str,
    image: String,
    thumb: Option<String>,
    at: Place,
    params: Params,
) -> GuiObject {
    GuiObject::Slider(Slider {
        id: Some(format!("wsz.{id}")),
        action: Some(action.to_owned()),
        image: Some(image),
        downimage: None,
        hoverimage: None,
        thumbimage: thumb,
        rect: at.rect(),
        params,
    })
}

/// Shared `params` extras for the three horizontal sliders (pressed-thumb
/// id, when the skin provided one).
fn thumb_down_param(down: Option<String>) -> Params {
    let mut params = Params::new();
    if let Some(down) = down {
        params.insert("thumbdownimage".to_owned(), down);
    }
    params
}

// ---- main (player) window ---------------------------------------------------

/// Synthesizes the `main` container: background, transport buttons, toggles,
/// sliders, displays, indicators and visualizer.
fn main_window(out: &mut Builder, viscolors: &VisColors) {
    out.container("main", "Main Window");
    let mut objects = Vec::new();

    // Background. A missing main.bmp is tolerated (warned) — the window and
    // its controls still exist, on a transparent background.
    if let Some(bg) = out.whole_bitmap("main", "main.bmp") {
        objects.push(layer("main.bg", bg, 0, 0, WINDOW_W, WINDOW_H));
    }
    // The player's titlebar graphic lives in main.bmp's top 14 rows and
    // carries the Winamp wordmark (see the layout table above). main.bmp as
    // a whole stays a raster payload — its dithered body is huge as vector
    // geometry — so re-emit the flat band as its OWN bitmap: the payload
    // pipeline can then ship it as SVG and keep the wordmark crisp at
    // fractional UI scales (1.25×/1.5×). At 1× it is a pixel-identical
    // overlay of the band underneath.
    //
    // The crop is clamped to the decoded bitmap on BOTH axes: `bitmap()`
    // does not validate rects (payloads would drop an out-of-range crop with
    // a per-load CropOutOfRange warning), and a skin with a narrower or
    // shorter main.bmp should simply get a smaller band — or none at all.
    let band_size = crate::images::file_dimensions(out.archive, "main.bmp")
        .map(|(width, height)| (width.min(WINDOW_W as u32), height.min(14)))
        .filter(|(width, height)| *width > 0 && *height > 0);
    if let Some((width, height)) = band_size
        && let Some(band) = out.bitmap("main.titlebar", "main.bmp", 0, 0, width, height)
    {
        objects.push(layer(
            "main.titlebar",
            band,
            0,
            0,
            width as i32,
            height as i32,
        ));
    }

    // Songticker + time-display fonts. numbers.bmp is the modern form;
    // num0.bmp..num9.bmp whole-digit files are the older alternative. Each
    // strip is ALSO emitted as a whole-file bitmap (`wsz.font.*.image`) so
    // `bitmap_payloads` materializes it for the renderer's glyph blitter
    // (probed silently — `bitmap_font` already warned if the file is gone).
    let text_font = out.bitmap_font("font.text", "text.bmp", 5, 6);
    if text_font.is_some() {
        out.present_bitmap("font.text.image", "text.bmp");
    }
    let numbers_font = out.bitmap_font("font.numbers", "numbers.bmp", 9, 13);
    if numbers_font.is_some() {
        out.present_bitmap("font.numbers.image", "numbers.bmp");
    }

    // Transport buttons. Strip layout: normal row y=0, pressed row y=18 (the
    // strip is 36px tall); prev/play/pause/stop are 23px cells, next/eject
    // 22px. Eject sits one pixel lower than its siblings (y=89, not 88).
    let transports: [(&str, &str, i32, i32, u32, u32, &str); 6] = [
        // (id, action, window-x, window-y, strip-x, width, tooltip)
        ("btn.prev", "PREV", 16, 88, 0, 23, "Previous Track"),
        ("btn.play", "PLAY", 39, 88, 23, 23, "Play"),
        ("btn.pause", "PAUSE", 62, 88, 46, 23, "Pause"),
        ("btn.stop", "STOP", 85, 88, 69, 23, "Stop"),
        // next is 22px wide in every reference implementation, not 23
        ("btn.next", "NEXT", 108, 88, 92, 22, "Next Track"),
        ("btn.eject", "EJECT", 136, 89, 114, 22, "Open File(s)"),
    ];
    for (id, action, wx, wy, sx, w, tooltip) in transports {
        if let (Some(image), Some(down)) = (
            out.bitmap(id, "cbuttons.bmp", sx, 0, w, 16),
            out.bitmap(&format!("{id}.down"), "cbuttons.bmp", sx, 18, w, 16),
        ) {
            objects.push(button(
                id,
                action,
                image,
                Some(down),
                place(wx, wy, w as i32, 16),
                tooltip,
            ));
        }
    }

    // Seek slider: posbar.bmp = 248px trough + normal (x=248) / pressed
    // (x=278) thumbs.
    let trough = out.bitmap("seek.bg", "posbar.bmp", 0, 0, 248, 10);
    let thumb = out.bitmap("seek.thumb", "posbar.bmp", 248, 0, 29, 10);
    let thumb_down = out.bitmap("seek.thumb.down", "posbar.bmp", 278, 0, 29, 10);
    if let (Some(trough), Some(thumb)) = (trough, thumb) {
        objects.push(slider(
            "seek",
            "SEEK",
            trough,
            Some(thumb),
            place(16, 72, 248, 10),
            thumb_down_param(thumb_down),
        ));
    }

    // Volume + balance. Each strip holds 28 trough frames of 15px plus the
    // thumbs at y=422; the on-screen controls are 13px tall, so frame 0 is
    // cropped to 68×13. Balance frames start at x=9 (the leading 9px is the
    // flat "no balance" strip); its centered rest state is frame 13.
    let trough = out.bitmap("vol.bg", "volume.bmp", 0, 0, 68, 13);
    let thumb = out.bitmap("vol.thumb", "volume.bmp", 15, 422, 14, 11);
    let thumb_down = out.bitmap("vol.thumb.down", "volume.bmp", 0, 422, 14, 11);
    if let (Some(trough), Some(thumb)) = (trough, thumb) {
        objects.push(slider(
            "vol",
            "VOLUME",
            trough,
            Some(thumb),
            place(107, 57, 68, 13),
            thumb_down_param(thumb_down),
        ));
    }
    let trough = out.bitmap("bal.bg", "balance.bmp", 9, 195, 38, 13);
    let thumb = out.bitmap("bal.thumb", "balance.bmp", 15, 422, 14, 11);
    let thumb_down = out.bitmap("bal.thumb.down", "balance.bmp", 0, 422, 14, 11);
    if let (Some(trough), Some(thumb)) = (trough, thumb) {
        objects.push(slider(
            "bal",
            "BALANCE",
            trough,
            Some(thumb),
            place(177, 57, 38, 13),
            thumb_down_param(thumb_down),
        ));
    }

    // Shuffle/repeat/EQ/playlist toggles. The base skin packs all four into
    // shufrep.bmp (three states each: off / pressed / active); skins that
    // ship the older separate shuffle.bmp / repeat.bmp / eq.bmp / pl.bmp
    // files are mapped file-by-file instead.
    push_toggle(
        out,
        &mut objects,
        "shuffle",
        "TOGGLE_SHUFFLE",
        "Shuffle",
        place(164, 89, 46, 15),
    );
    push_toggle(
        out,
        &mut objects,
        "repeat",
        "TOGGLE_REPEAT",
        "Repeat",
        place(210, 89, 28, 15),
    );
    push_toggle(
        out,
        &mut objects,
        "eq",
        "TOGGLE_EQ",
        "Equalizer",
        place(219, 58, 23, 12),
    );
    push_toggle(
        out,
        &mut objects,
        "pl",
        "TOGGLE_PLAYLIST",
        "Playlist Editor",
        place(242, 58, 23, 12),
    );

    // Songticker. The classic region is x=111..266 at y=27; its glyphs are
    // one line of 5px characters from text.bmp (rendered by the renderer's
    // bitmap-font path — hence the `font`/`charset` params).
    let mut ticker_params = Params::new();
    if let Some(font) = &text_font {
        ticker_params.insert("font".to_owned(), font.clone());
    }
    ticker_params.insert("charset".to_owned(), "classic-text".to_owned());
    ticker_params.insert("ticker".to_owned(), "1".to_owned());
    objects.push(GuiObject::Text(Text {
        id: Some("wsz.songticker".to_owned()),
        display: Some("songticker".to_owned()),
        rect: rect(111, 27, 155, 9),
        params: ticker_params,
    }));

    // Time display: one text object covering the digit region (x=36..111 at
    // y=26 — the brief's "y=39" is the region's bottom edge; the 13px digits
    // draw at y=26..39 in every reference implementation). Glyphs come from
    // numbers.bmp (11 cells of 9px; `digitoffsets` are the four digit cells
    // at window x=48/60/78/90 — the ":" between minute and second is baked
    // into main.bmp, so nothing is drawn for it) or, on older skins,
    // whole-file num0.bmp..num9.bmp (wired through `params["numbitmaps"]`;
    // probed silently — most skins simply don't ship that dialect).
    let mut time_params = Params::new();
    if let Some(font) = &numbers_font {
        time_params.insert("font".to_owned(), font.clone());
        time_params.insert("charset".to_owned(), "classic-numbers".to_owned());
        time_params.insert("digitoffsets".to_owned(), "12,24,42,54".to_owned());
    }
    if find_file(out.archive, "num0.bmp").is_some() {
        let digit_ids = (0..=9)
            .filter_map(|d| out.present_bitmap(&format!("num.{d}"), &format!("num{d}.bmp")))
            .collect::<Vec<_>>();
        if !digit_ids.is_empty() {
            time_params.insert(
                "numbitmaps".to_owned(),
                digit_ids
                    .iter()
                    .enumerate()
                    .map(|(d, id)| format!("{d}={id}"))
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
    }
    objects.push(GuiObject::Text(Text {
        id: Some("wsz.time".to_owned()),
        display: Some("time".to_owned()),
        rect: rect(36, 26, 75, 13),
        params: time_params,
    }));

    // Play/pause/stop status indicator (playpaus.bmp cells at x=0/9/18).
    let play = out.bitmap("status.play", "playpaus.bmp", 0, 0, 9, 9);
    let pause = out.bitmap("status.pause", "playpaus.bmp", 9, 0, 9, 9);
    let stop = out.bitmap("status.stop", "playpaus.bmp", 18, 0, 9, 9);
    if play.is_some() || pause.is_some() || stop.is_some() {
        let mut params = Params::new();
        if let Some(id) = play {
            params.insert("playimage".to_owned(), id);
        }
        if let Some(id) = pause {
            params.insert("pauseimage".to_owned(), id);
        }
        if let Some(id) = stop {
            params.insert("stopimage".to_owned(), id);
        }
        objects.push(GuiObject::Status(Status {
            id: Some("wsz.playstatus".to_owned()),
            rect: rect(26, 28, 9, 9),
            params,
        }));
    }

    // Mono/stereo indicators: monoster.bmp's left column is stereo (29px),
    // its right column mono (27px); top row active, bottom row inactive.
    // (The brief's "stereo at x=258" would overflow the 275px window — 239
    // is the position every reference implementation agrees on.)
    push_indicator(out, &mut objects, "mono", place(212, 41, 27, 12), 29);
    push_indicator(out, &mut objects, "stereo", place(239, 41, 29, 12), 0);

    // Visualizer over the classic vis region, fed by viscolor.txt.
    let mut vis_params = Params::new();
    if !viscolors.entries.is_empty() {
        vis_params.insert("viscolors".to_owned(), viscolors.joined());
    }
    objects.push(GuiObject::Vis(Vis {
        id: Some("wsz.vis".to_owned()),
        rect: rect(24, 43, 72, 16),
        params: vis_params,
    }));

    // Bitrate / kHz readouts: the digit cells beside the tiny "kbps"/"khz"
    // labels that are baked into main.bmp (x=111..126 and 156..166, y=43 —
    // Webamp's #kbps/#khz rects). Semantics ride in the TYPED `display`
    // attribute ("bitrate"/"khz" — see the conventions table); the renderer
    // fills them from the engine's audio_state metadata (`params["display"]`
    // would collide with the typed field under serde's flatten, hence the
    // attribute).
    for (id, x, w) in [("bitrate", 111, 15), ("khz", 156, 10)] {
        let mut params = Params::new();
        if let Some(font) = &text_font {
            params.insert("font".to_owned(), font.clone());
            params.insert("charset".to_owned(), "classic-text".to_owned());
        }
        objects.push(GuiObject::Text(Text {
            id: Some(format!("wsz.{id}")),
            display: Some(id.to_owned()),
            rect: rect(x, 43, w, 9),
            params,
        }));
    }

    // System buttons + clutter bar from titlebar.bmp (glyph grid: menu/min/
    // close at x=0/9/18 in rows y=0 normal / y=9 pressed; shade glyphs at
    // (0,18)/(9,18)). The player's own titlebar graphic is part of main.bmp;
    // the titlebar.bmp strips at x=27 belong to the EQ/playlist windows.
    push_sysbutton(
        out,
        &mut objects,
        "sysmenu",
        "SYSMENU",
        place(6, 3, 9, 9),
        (0, 0),
        (0, 9),
    );
    push_sysbutton(
        out,
        &mut objects,
        "minimize",
        "MINIMIZE",
        place(244, 3, 9, 9),
        (9, 0),
        (9, 9),
    );
    push_sysbutton(
        out,
        &mut objects,
        "shade",
        "TOGGLE_SHADE",
        place(254, 3, 9, 9),
        (0, 18),
        (9, 18),
    );
    push_sysbutton(
        out,
        &mut objects,
        "close",
        "CLOSE",
        place(264, 3, 9, 9),
        (18, 0),
        (18, 9),
    );
    if let Some(clutter) = out.bitmap("clutterbar", "titlebar.bmp", 304, 0, 8, 43) {
        objects.push(layer("clutterbar", clutter, 10, 22, 8, 43));
    }

    out.finish_layout("normal", WINDOW_W, WINDOW_H, objects);

    main_window_shade(out, text_font.as_deref(), viscolors);
}

/// Windowshade strip for the main window (275×14): titlebar.bmp's shade row,
/// the mini 5×6 clock, the 38×5 analyzer well, the 17×7 position slider and
/// the transport/window buttons.
///
/// The classic shade row bakes the transport glyphs into its pixels, so each
/// transport button's image is a crop of the same strip at its hit rectangle:
/// the overlay is pixel-identical and the controls stay clickable (there is no
/// pressed-state art in the shade row, so `downimage` repeats the normal crop).
fn main_window_shade(out: &mut Builder, text_font: Option<&str>, viscolors: &VisColors) {
    // No shade row → no shade layout at all; the resolver then degrades the
    // window to `normal` instead of collapsing to an empty 14px strip.
    let Some(bg) = out.bitmap("main.shade", "titlebar.bmp", 27, 29, WINDOW_W as u32, 14) else {
        return;
    };
    let mut objects = vec![layer("main.shade", bg, 0, 0, WINDOW_W, 14)];

    // Mini clock: text.bmp's 5×6 glyphs at the classic shade position
    // (Webamp's `.mini-time`: left 127, top 4, 25×6). The 25px rect fits the
    // five "MM:SS" cells exactly; longer strings clip in the renderer.
    if let Some(font) = text_font {
        let mut params = Params::new();
        params.insert("font".to_owned(), font.to_owned());
        params.insert("charset".to_owned(), "classic-text".to_owned());
        objects.push(GuiObject::Text(Text {
            id: Some("wsz.shade.time".to_owned()),
            display: Some("time".to_owned()),
            rect: rect(127, 4, 25, 8),
            params,
        }));
    }

    // Mini analyzer well (Webamp's shade visualizer: 38×5 at 79,5 — the
    // renderer draws the live bars and clicking cycles bars → scope → off).
    let mut vis_params = Params::new();
    if !viscolors.entries.is_empty() {
        vis_params.insert("viscolors".to_owned(), viscolors.joined());
    }
    objects.push(GuiObject::Vis(Vis {
        id: Some("wsz.shade.vis".to_owned()),
        rect: rect(79, 5, 38, 5),
        params: vis_params,
    }));

    // Mini position slider (titlebar.bmp rows: 17×7 trough at (0,36), 3×7
    // thumb at (20,36)).
    let track = out.bitmap("shade.pos", "titlebar.bmp", 0, 36, 17, 7);
    let thumb = out.bitmap("shade.pos.thumb", "titlebar.bmp", 20, 36, 3, 7);
    if let (Some(track), Some(thumb)) = (track, thumb) {
        objects.push(slider(
            "shade.pos",
            "SEEK",
            track,
            Some(thumb),
            place(226, 4, 17, 7),
            Params::new(),
        ));
    }

    // Transport hit areas (Webamp's shade action positions). Each image is
    // cropped from the strip itself at the same window coordinates, so the
    // background art shows through unchanged.
    const TRANSPORT: [(&str, &str, i32, i32, i32, i32, &str); 6] = [
        ("shade.prev", "PREV", 169, 2, 7, 10, "Previous Track"),
        ("shade.play", "PLAY", 176, 2, 10, 10, "Play"),
        ("shade.pause", "PAUSE", 186, 2, 9, 10, "Pause"),
        ("shade.stop", "STOP", 195, 2, 9, 10, "Stop"),
        ("shade.next", "NEXT", 204, 2, 10, 10, "Next Track"),
        ("shade.eject", "EJECT", 215, 2, 10, 10, "Open File(s)"),
    ];
    for (id, action, x, y, w, h, tip) in TRANSPORT {
        let crop = out.bitmap(
            id,
            "titlebar.bmp",
            (27 + x) as u32,
            (29 + y) as u32,
            w as u32,
            h as u32,
        );
        if let Some(img) = crop {
            objects.push(button(
                id,
                action,
                img.clone(),
                Some(img),
                place(x, y, w, h),
                tip,
            ));
        }
    }

    // Window buttons: reuse the glyph bitmaps the normal layout emitted from
    // the same titlebar.bmp (this helper only runs when that file exists), so
    // no duplicate bitmap elements enter the payload.
    const SYSBUTTONS: [(&str, &str, i32, i32, &str); 4] = [
        ("sysmenu", "SYSMENU", 6, 3, "Winamp Menu"),
        ("minimize", "MINIMIZE", 244, 3, "Minimize"),
        ("shade", "TOGGLE_SHADE", 254, 3, "Toggle Windowshade"),
        ("close", "CLOSE", 264, 3, "Close"),
    ];
    for (id, action, x, y, tip) in SYSBUTTONS {
        objects.push(button(
            id,
            action,
            format!("wsz.{id}"),
            Some(format!("wsz.{id}.down")),
            place(x, y, 9, 9),
            tip,
        ));
    }

    out.finish_layout("shade", WINDOW_W, 14, objects);
}

/// One of the four toggle buttons that live in shufrep.bmp (or their
/// separate-file variants on older skins). shufrep.bmp cells: shuffle/repeat
/// in rows y=0/15/30, eq/pl in rows y=61 (off) / y=73 (active) with the
/// pressed cells beside the active ones.
fn push_toggle(
    out: &mut Builder,
    objects: &mut Vec<GuiObject>,
    id: &str,
    action: &str,
    tooltip: &str,
    at: Place,
) {
    let (off, pressed, active) = match id {
        "shuffle" => ((28, 0), (28, 15), (28, 30)),
        "repeat" => ((0, 0), (0, 15), (0, 30)),
        "eq" => ((0, 61), (46, 73), (0, 73)),
        _ => ((23, 61), (69, 73), (23, 73)),
    };
    let image = match out.bitmap(id, "shufrep.bmp", off.0, off.1, at.w as u32, at.h as u32) {
        Some(id) => id,
        // fall back to the separate per-control files of pre-2.x skins
        // (probed silently: their absence alongside shufrep.bmp is normal)
        None => match out.present_bitmap(id, &format!("{id}.bmp")) {
            Some(fallback) => fallback,
            None => return, // already warned; object skipped
        },
    };
    let down = out
        .bitmap(
            &format!("{id}.down"),
            "shufrep.bmp",
            pressed.0,
            pressed.1,
            at.w as u32,
            at.h as u32,
        )
        .or_else(|| out.present_bitmap(&format!("{id}.down"), &format!("{id}.down.bmp")));
    let active = out
        .bitmap(
            &format!("{id}.active"),
            "shufrep.bmp",
            active.0,
            active.1,
            at.w as u32,
            at.h as u32,
        )
        .or_else(|| out.present_bitmap(&format!("{id}.active"), &format!("{id}.active.bmp")));
    objects.push(toggle(id, action, image, down, active, at, tooltip));
}

/// A mono/stereo indicator: a `Status` object whose `params["display"]` is
/// `"mono"`/`"stereo"` (the channel-indicator labels — see the conventions
/// table) with `image` = the inactive (dim) and `activeimage` = the active
/// (lit) monoster.bmp cells. `strip_x` selects the indicator's column.
fn push_indicator(
    out: &mut Builder,
    objects: &mut Vec<GuiObject>,
    id: &str,
    at: Place,
    strip_x: u32,
) {
    let image = out.bitmap(
        &format!("{id}.off"),
        "monoster.bmp",
        strip_x,
        12,
        at.w as u32,
        12,
    );
    let active = out.bitmap(
        &format!("{id}.on"),
        "monoster.bmp",
        strip_x,
        0,
        at.w as u32,
        12,
    );
    if image.is_none() && active.is_none() {
        return;
    }
    let mut params = Params::new();
    params.insert("display".to_owned(), id.to_owned());
    if let Some(id) = image {
        params.insert("image".to_owned(), id);
    }
    if let Some(id) = active {
        params.insert("activeimage".to_owned(), id);
    }
    objects.push(GuiObject::Status(Status {
        id: Some(format!("wsz.{id}")),
        rect: at.rect(),
        params,
    }));
}

/// A window-system button (menu / minimize / shade / close) from the
/// titlebar.bmp glyph grid; `glyph`/`down_glyph` are the (x, y) cells of
/// the normal/pressed states.
fn push_sysbutton(
    out: &mut Builder,
    objects: &mut Vec<GuiObject>,
    id: &str,
    action: &str,
    at: Place,
    glyph: (u32, u32),
    down_glyph: (u32, u32),
) {
    if let (Some(image), Some(down)) = (
        out.bitmap(id, "titlebar.bmp", glyph.0, glyph.1, 9, 9),
        out.bitmap(
            &format!("{id}.down"),
            "titlebar.bmp",
            down_glyph.0,
            down_glyph.1,
            9,
            9,
        ),
    ) {
        let tooltip = match action {
            "SYSMENU" => "Winamp Menu",
            "MINIMIZE" => "Minimize",
            "TOGGLE_SHADE" => "Toggle Windowshade",
            _ => "Close",
        };
        objects.push(button(id, action, image, Some(down), at, tooltip));
    }
}

// ---- EQ window --------------------------------------------------------------

/// Synthesizes the `eq` container: background, titlebar, ON/AUTO/PRESETS
/// buttons and the preamp + 10 band sliders (whose frequencies match
/// NaVinaMP's own EQ exactly).
fn eq_window(out: &mut Builder) {
    out.container("eq", "Equalizer");
    let mut objects = Vec::new();

    if let Some(bg) = out.bitmap("eq.bg", "eqmain.bmp", 0, 0, 275, 116) {
        objects.push(layer("eq.bg", bg, 0, 0, WINDOW_W, WINDOW_H));
    }
    // EQ titlebar strip (active row y=134; the inactive row y=149 rides in
    // params, mirroring how the Wasabi parser preserves `inactiveimage`).
    // The id is kept for the windowshade layout's fallback strip.
    let titlebar = out.bitmap("eq.titlebar", "eqmain.bmp", 0, 134, 275, 14);
    if let Some(bar) = &titlebar {
        let inactive = out.bitmap("eq.titlebar.off", "eqmain.bmp", 0, 149, 275, 14);
        let mut params = Params::new();
        if let Some(id) = inactive {
            params.insert("inactiveimage".to_owned(), id);
        }
        objects.push(GuiObject::Layer(Layer {
            id: Some("wsz.eq.titlebar".to_owned()),
            image: Some(bar.clone()),
            rect: rect(0, 0, WINDOW_W, 14),
            params,
        }));
    }
    // The close glyph is painted into the titlebar strip at the far right;
    // overlay a pixel-identical 9×9 crop so it is actually clickable. There
    // is no separate pressed-state art in eqmain.bmp, so reuse the crop.
    // eqmain.bmp also supplies eq.bg/eq.titlebar, so a missing file already
    // removes the whole EQ surface — no separate button fallback is needed.
    if let Some(close) = out.bitmap("eq.close", "eqmain.bmp", 264, 137, 9, 9) {
        objects.push(button(
            "eq.close",
            "CLOSE",
            close.clone(),
            Some(close),
            place(264, 3, 9, 9),
            "Close",
        ));
    }

    // ON / AUTO / PRESETS. (The brief's "AUTO at x=48" is off by nine: the
    // 33px button starts at x=39, right after the 25px ON button at x=14.)
    let on = out.bitmap("eq.on", "eqmain.bmp", 10, 119, 25, 12);
    let on_down = out.bitmap("eq.on.down", "eqmain.bmp", 128, 119, 25, 12);
    let on_active = out.bitmap("eq.on.active", "eqmain.bmp", 69, 119, 25, 12);
    if let (Some(on), Some(down), Some(active)) = (on, on_down, on_active) {
        objects.push(toggle(
            "eq.on",
            "EQ_TOGGLE",
            on,
            Some(down),
            Some(active),
            place(14, 18, 25, 12),
            "EQ On/Off",
        ));
    }
    let auto = out.bitmap("eq.auto", "eqmain.bmp", 35, 119, 33, 12);
    let auto_down = out.bitmap("eq.auto.down", "eqmain.bmp", 153, 119, 33, 12);
    let auto_active = out.bitmap("eq.auto.active", "eqmain.bmp", 94, 119, 33, 12);
    if let (Some(auto), Some(down), Some(active)) = (auto, auto_down, auto_active) {
        objects.push(toggle(
            "eq.auto",
            "EQ_AUTO",
            auto,
            Some(down),
            Some(active),
            place(39, 18, 33, 12),
            "Auto",
        ));
    }
    let presets = out.bitmap("eq.presets", "eqmain.bmp", 224, 164, 44, 12);
    let presets_down = out.bitmap("eq.presets.down", "eqmain.bmp", 224, 176, 44, 12);
    if let (Some(presets), Some(down)) = (presets, presets_down) {
        objects.push(button(
            "eq.presets",
            "EQ_PRESETS",
            presets,
            Some(down),
            place(217, 18, 44, 12),
            "Presets",
        ));
    }

    // Curve graph background (the live curve itself is renderer-drawn).
    if let Some(graph) = out.bitmap("eq.graph", "eqmain.bmp", 0, 294, 113, 19) {
        objects.push(layer("eq.graph", graph, 86, 17, 113, 19));
    }

    // 11 vertical sliders: preamp + 10 bands, one shared thumb ((0,164)
    // normal, (0,176) pressed) and the flat 0dB trough at (13,229). Track
    // x-positions: preamp at 21, band i at 78+18i.
    let track = out.bitmap("eq.track", "eqmain.bmp", 13, 229, 14, 63);
    let thumb = out.bitmap("eq.thumb", "eqmain.bmp", 0, 164, 11, 11);
    let thumb_down = out.bitmap("eq.thumb.down", "eqmain.bmp", 0, 176, 11, 11);
    if let (Some(track), Some(thumb)) = (track, thumb) {
        let mut base = Params::new();
        base.insert("orientation".to_owned(), "vertical".to_owned());
        if let Some(down) = thumb_down {
            base.insert("thumbdownimage".to_owned(), down);
        }

        let mut preamp = base.clone();
        preamp.insert("index".to_owned(), "0".to_owned());
        preamp.insert("band".to_owned(), "preamp".to_owned());
        objects.push(slider(
            "eq.preamp",
            "EQ_SET",
            track.clone(),
            Some(thumb.clone()),
            place(21, 38, 14, 63),
            preamp,
        ));

        for (i, hz) in EQ_BANDS_HZ.iter().enumerate() {
            let mut params = base.clone();
            params.insert("index".to_owned(), (i + 1).to_string());
            params.insert("band".to_owned(), hz.to_string());
            objects.push(slider(
                &format!("eq.band{i}"),
                "EQ_SET",
                track.clone(),
                Some(thumb.clone()),
                place(78 + 18 * i as i32, 38, 14, 63),
                params,
            ));
        }
    }

    out.finish_layout("normal", WINDOW_W, WINDOW_H, objects);

    eq_window_shade(out, titlebar.as_deref());
}

/// Windowshade strip for the EQ window (275×14).
///
/// Classic skins carry the real EQ shade art in the optional `eq_ex.bmp`
/// (selected row y=0, normal row y=15). Skins without it fall back to the
/// normal EQ titlebar strip from `eqmain.bmp`, which still shows the classic
/// "EQ" bar. The close/shade buttons reuse titlebar.bmp's glyph grid.
fn eq_window_shade(out: &mut Builder, titlebar: Option<&str>) {
    // Probe eq_ex.bmp silently: its absence is normal and handled by the
    // eqmain titlebar fallback (no ClassicMissingOptional warning).
    let bg = if find_file(out.archive, "eq_ex.bmp").is_some() {
        out.bitmap("eq.shade", "eq_ex.bmp", 0, 0, WINDOW_W as u32, 14)
    } else {
        titlebar.map(str::to_owned)
    };
    // No shade art at all → omit the layout so the resolver degrades to
    // `normal` instead of collapsing to an empty strip.
    let Some(bg) = bg else {
        return;
    };
    let mut objects = vec![layer("eq.shade", bg, 0, 0, WINDOW_W, 14)];

    push_sysbutton(
        out,
        &mut objects,
        "eq.shade.toggle",
        "TOGGLE_SHADE",
        place(254, 3, 9, 9),
        (0, 18),
        (9, 18),
    );
    push_sysbutton(
        out,
        &mut objects,
        "eq.shade.close",
        "CLOSE",
        place(264, 3, 9, 9),
        (18, 0),
        (18, 9),
    );

    out.finish_layout("shade", WINDOW_W, 14, objects);
}

// ---- playlist window --------------------------------------------------------

/// Classic playlist geometry (275×232), cross-checked against Webamp:
///
/// - titlebar 0,0 275×20: 25px left corner, a repeating 25×20 tile, the
///   100×20 "WINAMP PLAYLIST" title, 25px right corner (shade/close glyphs)
/// - left rail 0,20 12×174; right rail 255,20 20×174 (beveled side art)
/// - bottom bar 0,194 275×38: left 125 (ADD/REM/SEL/MISC baked in), right
///   150 (mini transport + LED + LIST OPTS + scroll arrows)
///
/// The list rows are renderer-drawn HTML using pledit.txt's colors, so the
/// container carries invisible markers for the list body/totals readout and
/// transparent hotspots over the baked-in buttons and titlebar glyphs.
/// No container is emitted when `pledit.bmp` is absent or too small for the
/// frame crops — the frontend then keeps its palette-themed fallback panel.
fn playlist_window(out: &mut Builder) {
    let Some(actual) = find_file(out.archive, "pledit.bmp") else {
        out.missing("pledit.bmp", "playlist window");
        return;
    };
    // The frame crops reach x=276 / y=110; a truncated or placeholder
    // pledit.bmp would otherwise emit eight out-of-range crops and a skinned
    // window with no visible frame. Keep the HTML fallback instead.
    match image_size(out.archive, &actual) {
        Some((w, h)) if w >= 276 && h >= 111 => {}
        Some((w, h)) => {
            out.warnings.push(Warning::MalformedValue {
                element: "pledit.bmp".to_owned(),
                attribute: "size".to_owned(),
                value: format!("{w}x{h}"),
            });
            return;
        }
        None => {
            out.warnings.push(Warning::ImageDecode {
                file: "pledit.bmp".to_owned(),
                message: "cannot decode the playlist frame".to_owned(),
            });
            return;
        }
    }

    let mut objects = Vec::new();

    // Titlebar (active row y=0; the inactive row y=21 is unused because the
    // app does not track cross-window focus in the skin).
    if let Some(image) = out.bitmap("pl.tl", "pledit.bmp", 0, 0, 25, 20) {
        objects.push(layer("pl.tl", image, 0, 0, 25, 20));
    }
    if let Some(image) = out.bitmap("pl.tc", "pledit.bmp", 127, 0, 25, 20) {
        objects.push(tiled_layer(
            "pl.tc",
            image,
            place(25, 0, 225, 20),
            "x",
            Params::new(),
        ));
    }
    if let Some(image) = out.bitmap("pl.title", "pledit.bmp", 26, 0, 100, 20) {
        objects.push(layer("pl.title", image, 87, 0, 100, 20));
    }
    if let Some(image) = out.bitmap("pl.tr", "pledit.bmp", 153, 0, 25, 20) {
        objects.push(layer("pl.tr", image, 250, 0, 25, 20));
    }

    // Side rails (the list itself is the transparent 12..255 × 20..194 hole).
    if let Some(image) = out.bitmap("pl.ml", "pledit.bmp", 0, 42, 12, 29) {
        objects.push(tiled_layer(
            "pl.ml",
            image,
            place(0, 20, 12, 174),
            "y",
            Params::new(),
        ));
    }
    if let Some(image) = out.bitmap("pl.mr", "pledit.bmp", 31, 42, 20, 29) {
        objects.push(tiled_layer(
            "pl.mr",
            image,
            place(255, 20, 20, 174),
            "y",
            Params::new(),
        ));
    }

    // Bottom bar.
    if let Some(image) = out.bitmap("pl.bl", "pledit.bmp", 0, 72, 125, 38) {
        objects.push(layer("pl.bl", image, 0, 194, 125, 38));
    }
    if let Some(image) = out.bitmap("pl.bc", "pledit.bmp", 126, 72, 150, 38) {
        objects.push(layer("pl.bc", image, 125, 194, 150, 38));
    }

    // A 1×1 / nonstandard pledit.bmp yields zero frame pieces; keep the
    // HTML fallback panel instead of an empty skinned window.
    if objects.is_empty() {
        return;
    }

    out.container("playlist", "Playlist Editor");

    // Renderer slots (no bitmap): HTML list body, totals readout and the
    // titlebar drag band.
    let mut list = marker("pl.list", place(12, 20, 243, 174));
    if let GuiObject::Layer(layer) = &mut list {
        // Classic PE rows are 16px in the 174px list body.
        layer.params.insert("rowheight".to_owned(), "16".to_owned());
    }
    objects.push(list);
    objects.push(marker("pl.totals", place(126, 202, 68, 11)));
    objects.push(marker("playlist.titlebar.zone", place(0, 0, WINDOW_W, 20)));

    // Baked-in glyphs become real controls.
    objects.push(hotspot(
        "pl.shade",
        "TOGGLE_SHADE",
        place(254, 3, 9, 9),
        "Window shade",
    ));
    objects.push(hotspot("pl.close", "CLOSE", place(264, 3, 9, 9), "Close"));
    objects.push(hotspot(
        "pl.add",
        "PLAYLIST_ADD",
        place(14, 202, 22, 18),
        "Add media",
    ));
    objects.push(hotspot(
        "pl.rem",
        "PLAYLIST_REMOVE",
        place(43, 202, 22, 18),
        "Remove selected",
    ));
    objects.push(hotspot(
        "pl.sel",
        "PLAYLIST_SELECT",
        place(72, 202, 22, 18),
        "Select playing track",
    ));
    objects.push(hotspot(
        "pl.misc",
        "PLAYLIST_MISC",
        place(101, 202, 22, 18),
        "Playlist options",
    ));
    objects.push(hotspot(
        "pl.listopts",
        "PLAYLIST_LIST",
        place(231, 202, 22, 18),
        "Playlist options",
    ));

    out.finish_layout("normal", WINDOW_W, PLAYLIST_H, objects);
}

// ---- viscolor.txt -----------------------------------------------------------

/// Parsed `viscolor.txt`: up to 24 `R,G,B` lines (trailing `// comments` are
/// the norm) driving the classic analyzer/oscilloscope palette.
struct VisColors {
    entries: Vec<[u8; 3]>,
}

impl VisColors {
    fn parse(text: &str) -> Self {
        let mut entries = Vec::new();
        for line in text.lines() {
            if entries.len() == 24 {
                break; // spec is 24 lines; extras are skin junk
            }
            if let Some(rgb) = parse_rgb_line(line) {
                entries.push(rgb);
            }
        }
        Self { entries }
    }

    /// `r,g,b;r,g,b;…` — semicolon-joined because the entries themselves
    /// contain commas. Carried by the vis object in `params["viscolors"]`.
    fn joined(&self) -> String {
        self.entries
            .iter()
            .map(|c| format!("{},{},{}", c[0], c[1], c[2]))
            .collect::<Vec<_>>()
            .join(";")
    }
}

/// Parses the first three comma-separated integers of a line as `r,g,b`.
/// Fields may carry leading spaces (`150, 150, 150`) and the last one a
/// trailing comment (`24,33,41, // grey`); lines with fewer than three
/// fields are rejected.
fn parse_rgb_line(line: &str) -> Option<[u8; 3]> {
    let mut parts = line.split(',');
    let mut out = [0u8; 3];
    for slot in &mut out {
        let part = parts.next()?.trim_start();
        let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            return None;
        }
        *slot = digits.parse().ok()?;
    }
    Some(out)
}

/// `[r, g, b]` → `#rrggbb` (lowercase).
fn rgb_hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

// ---- pledit.txt -------------------------------------------------------------

/// Parsed `pledit.txt` playlist colors. Two dialects exist:
///
/// - the modern INI form shipped with 2.9x skins: a `[Text]` header plus
///   `Normal=#00FF00` / `Current=…` / `NormalBG=…` / `OldBG=…` /
///   `SelectedBG=…` / `Font=Arial`
/// - the ancient (Winamp 1.x) form: a junk header line followed by bare
///   `R,G,B` lines in the documented order normal, normal-bg, current,
///   current-bg, selected, selected-bg.
///
/// Keys are normalized to `normal`, `current`, `normalbg`, `oldbg`,
/// `selectedbg`, `selected`; values to `#rrggbb` hex (bare-line values are
/// converted from decimal).
struct PleditColors {
    /// Insertion-ordered (key, `#rrggbb`) pairs.
    colors: Vec<(String, String)>,
    font: Option<String>,
}

impl PleditColors {
    fn parse(text: &str, warnings: &mut Vec<Warning>) -> Self {
        let mut colors = Vec::new();
        let mut font = None;
        let mut bare_lines = 0usize;

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('[') {
                continue; // `[Text]` header / section markers
            }
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim().to_ascii_lowercase();
                match key.as_str() {
                    "normal" | "current" | "normalbg" | "oldbg" | "selectedbg" | "selected" => {
                        match parse_hex_color(value.trim()) {
                            Some(hex) => colors.push((key, hex)),
                            None => warnings.push(Warning::MalformedValue {
                                element: "pledit.txt".to_owned(),
                                attribute: key,
                                value: value.trim().to_owned(),
                            }),
                        }
                    }
                    "font" => font = Some(value.trim().to_owned()),
                    _ => {} // non-color keys (TitleFont, …) are not needed
                }
                continue;
            }
            if let Some(rgb) = parse_rgb_line(line) {
                // Ancient bare `R,G,B` dialect, in the documented order.
                const ORDER: [&str; 6] = [
                    "normal",
                    "normalbg",
                    "current",
                    "currentbg",
                    "selected",
                    "selectedbg",
                ];
                if bare_lines < ORDER.len() {
                    // current-bg has no modern key; fold it into oldbg
                    let key = match ORDER[bare_lines] {
                        "currentbg" => "oldbg",
                        other => other,
                    };
                    colors.push((key.to_owned(), rgb_hex(rgb)));
                }
                bare_lines += 1;
            }
            // anything else is junk (headers vary between skins) — expected
        }
        Self { colors, font }
    }
}

/// `#RRGGBB` / `#rrggbb` / `RRGGBB` → normalized `#rrggbb`.
fn parse_hex_color(value: &str) -> Option<String> {
    let hex = value.strip_prefix('#').unwrap_or(value);
    if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(format!("#{}", hex.to_ascii_lowercase()))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_lines_parse_with_trailing_comments() {
        assert_eq!(
            parse_rgb_line("24,33,41, // color 1 = grey for dots"),
            Some([24, 33, 41])
        );
        assert_eq!(parse_rgb_line("0,0,0,"), Some([0, 0, 0]));
        assert_eq!(
            parse_rgb_line("150, 150, 150, // 23"),
            Some([150, 150, 150])
        );
        assert_eq!(parse_rgb_line("not a color"), None);
        assert_eq!(parse_rgb_line("1,2"), None);
        assert_eq!(rgb_hex([0, 255, 0]), "#00ff00");
    }

    #[test]
    fn viscolor_keeps_at_most_24_entries() {
        let text = (0..30)
            .map(|i| format!("{i},{i},{i}, // c"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(VisColors::parse(&text).entries.len(), 24);
    }

    #[test]
    fn pledit_modern_ini_dialect_parses() {
        let text = "[Text]\nNormal=#00FF00\nCurrent=#FFFFFF\nNormalBG=#000000\nSelectedBG=#0000C6\nFont=Arial\n";
        let parsed = PleditColors::parse(text, &mut Vec::new());
        assert_eq!(
            parsed.colors,
            vec![
                ("normal".to_owned(), "#00ff00".to_owned()),
                ("current".to_owned(), "#ffffff".to_owned()),
                ("normalbg".to_owned(), "#000000".to_owned()),
                ("selectedbg".to_owned(), "#0000c6".to_owned()),
            ]
        );
        assert_eq!(parsed.font.as_deref(), Some("Arial"));
    }

    #[test]
    fn pledit_ancient_bare_lines_parse() {
        let text = "Recently played?\n0,255,0\n0,0,0\n255,255,255\n10,10,40\n";
        let parsed = PleditColors::parse(text, &mut Vec::new());
        assert_eq!(
            parsed.colors,
            vec![
                ("normal".to_owned(), "#00ff00".to_owned()),
                ("normalbg".to_owned(), "#000000".to_owned()),
                ("current".to_owned(), "#ffffff".to_owned()),
                ("oldbg".to_owned(), "#0a0a28".to_owned()),
            ]
        );
    }

    #[test]
    fn bad_pledit_color_warns() {
        let text = "[Text]\nNormal=green-ish\n";
        let mut warnings = Vec::new();
        let parsed = PleditColors::parse(text, &mut warnings);
        assert!(parsed.colors.is_empty());
        assert_eq!(
            warnings,
            vec![Warning::MalformedValue {
                element: "pledit.txt".to_owned(),
                attribute: "normal".to_owned(),
                value: "green-ish".to_owned(),
            }]
        );
    }
}
