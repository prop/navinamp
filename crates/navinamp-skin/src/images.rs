//! Bitmap payload extraction: `<bitmap>` elements → image data-URLs.
//!
//! The parsing half ([`Skin`](crate::Skin)) keeps `elements.bitmaps` as
//! *references* to files inside the archive; this module materializes them:
//! every bitmap with a `file` is read (case-insensitively), decoded as
//! PNG/BMP, cropped to its `x`/`y`/`w`/`h` sub-rectangle when given, given
//! Wasabi color-key transparency when it carries a `transparent` attribute,
//! and re-encoded as a data-URL the webview can drop into an `<img>`.
//!
//! Two encodings compete per bitmap and the smaller one wins:
//!
//! - **SVG** (pixel-faithful): every maximal same-colour rectangle becomes a
//!   subpath in that colour's `<path>`. Classic skins are pixel art and the
//!   renderer scales the whole surface with a CSS transform; nearest-neighbour
//!   upscaling is only faithful at integer factors, while 1.25×/1.5× produce
//!   alternating 1px/2px steps. Exact geometry scales cleanly at *any* factor —
//!   pixel-identical at 1×/2× and properly anti-aliased in between. Flat art
//!   (buttons, sliders, titlebars, borders) compresses well this way.
//!
//!   Corollary: adjacent *different-coloured* rectangles share an edge, and at
//!   fractional scales that edge is anti-aliased (blended). One-pixel outlines
//!   therefore read slightly softer than a hard-edged render — that AA is
//!   precisely what replaces nearest-neighbour's uneven pixel steps, not a
//!   regression. (`shape-rendering="crispEdges"` would restore hard edges at
//!   the cost of the smooth fractional scaling.)
//! - **PNG**: dithered/gradient artwork has almost no same-colour runs, so
//!   per-pixel geometry explodes (base-2-91's main window is ~20x larger as
//!   SVG); those bitmaps keep the raster payload.
//!
//! Exception to the contest: bitmaps whose file backs a `<bitmapfont>` glyph
//! strip always stay raster. The renderer blits them 1:1 into a canvas and
//! upscales the result chunky by design, so a vector strip buys no crispness
//! while adding an SVG rasterisation step to every glyph draw.
//!
//! Both encodings are 1:1 with the skin's own pixels; every scale factor is
//! the renderer's job (plan §3).
//!
//! Like the rest of the parser nothing here is fatal: a missing file,
//! undecodable bytes, an out-of-range crop or an unparseable transparency
//! value only costs a [`Warning`] — that bitmap simply gets no payload and
//! the renderer skips it.

use std::collections::HashMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use image::RgbaImage;

use crate::archive::SkinArchive;
use crate::error::Warning;
use crate::model::{Params, Skin};

/// Prefix of the vector payloads this module produces.
const SVG_PREFIX: &str = "data:image/svg+xml;base64,";
/// Prefix of the raster payloads this module produces.
const PNG_PREFIX: &str = "data:image/png;base64,";

/// How much larger than the PNG payload an SVG payload may be and still win.
///
/// SVG is the better *renderer* at fractional scales, so it gets a size
/// allowance; beyond it the vector geometry is a losing trade. 7× was picked
/// against the bundled base skin: it vectorizes 81 of 90 bitmaps (650 KiB
/// total vs 167 KiB as PNG) while the two large dithered gradient bitmaps —
/// which alone would add ~230 KiB each — stay raster.
const SVG_SIZE_BUDGET: f64 = 7.0;

/// Bitmaps that always ship as SVG even when over [`SVG_SIZE_BUDGET`].
///
/// The player titlebar band (`wsz.main.titlebar`, emitted by the classic
/// synthesizer only — this list is keyed on its generated id) is a 275×14
/// dithered strip carrying the Winamp wordmark. Its vector form is 2.6× the
/// PNG on clean bands and ~7–9× on noisy ones — the budget's target case —
/// but the absolute cost is bounded at ~75 KiB for a worst-case band (the
/// bundled base skin measures 34 KiB), and vectorizing it is what keeps the
/// wordmark crisp at fractional UI scales.
const VECTOR_FORCED: &[&str] = &["wsz.main.titlebar"];

/// Decoded pixel dimensions of one archive file (case-insensitive lookup),
/// when it decodes. The synthesizer clamps crops it emits with this — a
/// `bitmap()` rect is only validated later, in [`bitmap_payloads`], and an
/// out-of-range crop would otherwise warn on every skin load.
pub(crate) fn file_dimensions(archive: &SkinArchive, file: &str) -> Option<(u32, u32)> {
    let bytes = archive.read_file(file).ok()?;
    let decoded = image::load_from_memory(&bytes).ok()?;
    Some((decoded.width(), decoded.height()))
}

/// Materializes every `<bitmap>` that references a file into an image
/// data-URL (SVG or PNG, whichever is smaller — see the module docs).
///
/// Returns `(bitmap-id -> data-URL, warnings)`. Bitmaps are processed in
/// model order and on duplicate ids the **first** payload wins, mirroring
/// the parser's `Warning::DuplicateId` first-definition-wins rule. Entries
/// without a usable `file` attribute are skipped silently (the parser
/// already warns about those).
///
/// A note on duplicates: [`Skin::from_archive`](crate::Skin::from_archive)
/// already reports missing bitmap files as [`Warning::MissingFile`]; this
/// function reports them again so it stays useful standalone — callers that
/// surface both warning lists may want to de-duplicate.
///
/// # Example
///
/// ```no_run
/// # use std::path::Path;
/// # use navinamp_skin::SkinArchive;
/// let archive = SkinArchive::open(Path::new("skin.wal"))?;
/// let (skin, _) = navinamp_skin::Skin::from_archive(&archive)?;
/// let (images, warnings) = navinamp_skin::bitmap_payloads(&archive, &skin);
/// // images: "player.background" -> "data:image/svg+xml;base64,PHN2ZyB4bWxu..."
/// # Ok::<(), navinamp_skin::SkinError>(())
/// ```
pub fn bitmap_payloads(
    archive: &SkinArchive,
    skin: &Skin,
) -> (HashMap<String, String>, Vec<Warning>) {
    // Glyph strips are blitted 1:1 into a canvas by the renderer's bitmap
    // font code and then upscaled chunky by design — vectorising them buys
    // no crispness and adds an SVG rasterisation step to every glyph draw.
    // Keep every bitmap whose file backs a <bitmapfont> raster (file names
    // are compared case-insensitively, like the archive lookup).
    let font_files: Vec<&str> = skin
        .elements
        .bitmap_fonts
        .iter()
        .filter_map(|font| font.file.as_deref())
        .collect();
    let mut images = HashMap::new();
    let mut warnings = Vec::new();

    for bitmap in &skin.elements.bitmaps {
        // First payload for an id wins (parse-time DuplicateId semantics).
        if images.contains_key(&bitmap.id) {
            continue;
        }
        let Some(file) = bitmap.file.as_deref().filter(|f| !f.is_empty()) else {
            continue; // nothing to materialize; the parser already warned
        };

        let bytes = match archive.read_file(file) {
            Ok(bytes) => bytes,
            Err(_) => {
                warnings.push(Warning::MissingFile {
                    file: file.to_owned(),
                    referenced_by: format!("bitmap `{}`", bitmap.id),
                });
                continue;
            }
        };
        let decoded = match image::load_from_memory(&bytes) {
            Ok(decoded) => decoded.to_rgba8(),
            Err(err) => {
                warnings.push(Warning::ImageDecode {
                    file: file.to_owned(),
                    message: err.to_string(),
                });
                continue;
            }
        };

        let mut rgba = match crop(decoded, bitmap, file, &mut warnings) {
            Some(rgba) => rgba,
            None => continue, // out-of-range rect: warning already recorded
        };

        if let Some(value) = param_ignore_case(&bitmap.params, "transparent") {
            match parse_color_key(value) {
                Some(key) => make_color_transparent(&mut rgba, key),
                None => warnings.push(Warning::MalformedValue {
                    element: "bitmap".to_owned(),
                    attribute: "transparent".to_owned(),
                    value: value.to_owned(),
                }),
            }
        }

        let svg_url = {
            let svg = encode_svg(&rgba);
            format!("{SVG_PREFIX}{}", STANDARD.encode(svg.as_bytes()))
        };
        let png_url = match encode_png(&rgba) {
            Ok(png) => format!("{PNG_PREFIX}{}", STANDARD.encode(&png)),
            // Re-encoding an RGBA8 buffer as PNG cannot realistically fail;
            // fall back to the vector payload rather than inventing a third
            // warning variant for the impossible case.
            Err(_) => svg_url.clone(),
        };
        // Glyph strips stay raster regardless of the size comparison.
        let raster_only = bitmap.file.as_deref().is_some_and(|file| {
            font_files
                .iter()
                .any(|font_file| font_file.eq_ignore_ascii_case(file))
        });
        let vector_forced = VECTOR_FORCED.contains(&bitmap.id.as_str());
        let url = if raster_only {
            png_url
        } else if vector_forced || svg_url.len() as f64 <= png_url.len() as f64 * SVG_SIZE_BUDGET {
            svg_url
        } else {
            png_url
        };
        images.insert(bitmap.id.clone(), url);
    }

    (images, warnings)
}

/// Crops to the bitmap's sub-rectangle. Absent coordinates default to the
/// image origin / lower-right corner, so `x`/`y`-only or `w`/`h`-only
/// combinations behave sensibly. `None` (plus a warning) when the rect does
/// not fit the decoded image.
fn crop(
    decoded: RgbaImage,
    bitmap: &crate::model::Bitmap,
    file: &str,
    warnings: &mut Vec<Warning>,
) -> Option<RgbaImage> {
    if bitmap.is_whole_image() {
        return Some(decoded);
    }
    let (image_width, image_height) = decoded.dimensions();
    let x = bitmap.x.unwrap_or(0);
    let y = bitmap.y.unwrap_or(0);
    // Absent extent means "up to the edge of the image".
    let w = bitmap.w.unwrap_or(image_width.saturating_sub(x));
    let h = bitmap.h.unwrap_or(image_height.saturating_sub(y));

    let fits = w > 0
        && h > 0
        && x <= image_width
        && w <= image_width - x
        && y <= image_height
        && h <= image_height - y;
    if !fits {
        warnings.push(Warning::CropOutOfRange {
            bitmap: bitmap.id.clone(),
            file: file.to_owned(),
            x,
            y,
            w,
            h,
            image_width,
            image_height,
        });
        return None;
    }
    Some(image::imageops::crop_imm(&decoded, x, y, w, h).to_image())
}

/// Makes every pixel whose RGB exactly matches `key` fully transparent
/// (alpha 0, color bytes kept). This is the Wasabi color-key rule classic
/// skins rely on — magic pink `255,0,255` on otherwise-opaque BMPs.
fn make_color_transparent(rgba: &mut RgbaImage, key: [u8; 3]) {
    for pixel in rgba.pixels_mut() {
        if pixel.0[..3] == key {
            pixel.0[3] = 0;
        }
    }
}

/// Case-insensitive `params` lookup; attribute keys keep their original
/// casing in the model, and skins are inconsistent about `transparent`.
fn param_ignore_case<'a>(params: &'a Params, key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
}

/// Parses a Wasabi transparency color key liberally:
///
/// - `#RRGGBB`, `RRGGBB`, `#RRGGBBAA`, `RRGGBBAA` (hex, case-insensitive;
///   the alpha byte of 8-digit forms is ignored for matching)
/// - `0x`-prefixed forms of the same
/// - comma-separated decimals, `R,G,B` or `R,G,B,A` (the RGBA form matches
///   the notation skins use for `<color>` values)
///
/// Anything else returns `None` (caller warns and proceeds without a key).
fn parse_color_key(value: &str) -> Option<[u8; 3]> {
    let trimmed = value.trim();
    let hexish = trimmed
        .strip_prefix('#')
        .or_else(|| trimmed.strip_prefix("0x"))
        .unwrap_or(trimmed);
    if hexish != trimmed {
        // A `#`/`0x` prefix means hex, whatever the length.
        return match hexish.len() {
            6 => Some(hex_rgb(hexish)),
            8 => Some(hex_rgb(&hexish[..6])), // RRGGBBAA: alpha ignored
            _ => None,
        };
    }
    let parts: Vec<&str> = trimmed.split(',').map(str::trim).collect();
    match parts.len() {
        3 | 4 => {
            let r = parts[0].parse().ok()?;
            let g = parts[1].parse().ok()?;
            let b = parts[2].parse().ok()?;
            Some([r, g, b])
        }
        // A bare 6/8-digit run of hex digits also counts as hex.
        _ if hexish.len() == 6 && hexish.chars().all(|c| c.is_ascii_hexdigit()) => {
            Some(hex_rgb(hexish))
        }
        _ if hexish.len() == 8 && hexish.chars().all(|c| c.is_ascii_hexdigit()) => {
            Some(hex_rgb(&hexish[..6]))
        }
        _ => None,
    }
}

/// `RRGGBB` (already validated hex) → `[r, g, b]`.
fn hex_rgb(hex: &str) -> [u8; 3] {
    [
        u8::from_str_radix(&hex[0..2], 16).expect("validated hex"),
        u8::from_str_radix(&hex[2..4], 16).expect("validated hex"),
        u8::from_str_radix(&hex[4..6], 16).expect("validated hex"),
    ]
}

/// Encodes an RGBA buffer as a pixel-faithful SVG document.
///
/// Every maximal rectangle of one colour is merged greedily (2-D run
/// merging) and emitted as a subpath of that colour's `<path>` — one element
/// per colour instead of one per rectangle, which keeps dithered pixel art
/// from exploding in `<rect>` tag overhead. The artwork keeps its exact
/// pixel geometry at *any* CSS scale: integer factors render pixel-identical
/// to the bitmap, fractional factors get true edges instead of the uneven
/// 1px/2px steps of nearest-neighbour upscaling. Fully transparent pixels
/// emit nothing; partial alpha becomes `fill-opacity`.
///
/// The explicit `width`/`height`/`viewBox` keep intrinsic-size consumers
/// working — the renderer measures `naturalWidth` for animated-layer frame
/// heuristics.
fn encode_svg(rgba: &RgbaImage) -> String {
    use std::collections::BTreeMap;
    use std::fmt::Write as _;

    let (w, h) = rgba.dimensions();
    let (wu, hu) = (w as usize, h as usize);

    // Rectangles (as SVG subpaths) grouped by colour, keyed by RGBA bytes so
    // the output is deterministic. Colours are few (classic skins are
    // paletted), rectangles are many.
    let mut groups: BTreeMap<[u8; 4], String> = BTreeMap::new();
    let pixel = |x: usize, y: usize| rgba.get_pixel(x as u32, y as u32).0;

    // Rectangles claimed by an earlier run; keeps the merge greedy-linear
    // instead of re-scanning the whole buffer per rectangle.
    let mut done = vec![false; wu * hu];
    for y in 0..hu {
        for x in 0..wu {
            if done[y * wu + x] {
                continue;
            }
            let color = pixel(x, y);

            // Widest same-colour run to the right on this row.
            let mut x1 = x + 1;
            while x1 < wu && !done[y * wu + x1] && pixel(x1, y) == color {
                x1 += 1;
            }

            // Grow downwards while the whole run repeats unchanged.
            let mut y1 = y + 1;
            'grow: while y1 < hu {
                for xx in x..x1 {
                    if done[y1 * wu + xx] || pixel(xx, y1) != color {
                        break 'grow;
                    }
                }
                y1 += 1;
            }

            for yy in y..y1 {
                done[yy * wu + x..yy * wu + x1].fill(true);
            }
            if color[3] == 0 {
                continue; // fully transparent: nothing to paint
            }

            let d = groups.entry(color).or_default();
            let _ = write!(d, "M{x} {y}h{}v{}h-{}z", x1 - x, y1 - y, x1 - x);
        }
    }

    let mut svg = String::with_capacity(groups.values().map(String::len).sum::<usize>() + 256);
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">"
    );
    for (color, d) in groups {
        let _ = write!(
            svg,
            "<path fill=\"#{r:02x}{g:02x}{b:02x}\"",
            r = color[0],
            g = color[1],
            b = color[2],
        );
        if color[3] != 255 {
            let _ = write!(svg, " fill-opacity=\"{:.3}\"", f32::from(color[3]) / 255.0);
        }
        let _ = write!(svg, " d=\"{d}\"/>");
    }
    svg.push_str("</svg>");
    svg
}

/// Encodes an RGBA buffer as PNG bytes (the raster fallback for artwork whose
/// vector form would be much larger — see the module docs).
fn encode_png(rgba: &RgbaImage) -> Result<Vec<u8>, image::ImageError> {
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(rgba.clone())
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
    Ok(png)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_keys_parse_liberally() {
        assert_eq!(parse_color_key("255,0,255"), Some([255, 0, 255]));
        assert_eq!(parse_color_key(" 255 , 0 , 255 "), Some([255, 0, 255]));
        assert_eq!(parse_color_key("16,32,64,128"), Some([16, 32, 64]));
        assert_eq!(parse_color_key("#FF00FF"), Some([255, 0, 255]));
        assert_eq!(parse_color_key("ff00ff"), Some([255, 0, 255]));
        assert_eq!(parse_color_key("#ff00ff80"), Some([255, 0, 255]));
        assert_eq!(parse_color_key("0xFF00FF"), Some([255, 0, 255]));
        // classic color notations that are not color keys
        assert_eq!(parse_color_key("pretty in pink"), None);
        assert_eq!(parse_color_key("1,2"), None);
        assert_eq!(parse_color_key(""), None);
        assert_eq!(parse_color_key("#FFF"), None);
    }

    #[test]
    fn param_lookup_ignores_key_casing() {
        let mut params = Params::new();
        params.insert("Transparent".to_owned(), "255,0,255".to_owned());
        assert_eq!(param_ignore_case(&params, "transparent"), Some("255,0,255"));
        let empty = Params::new();
        assert_eq!(param_ignore_case(&empty, "transparent"), None);
    }

    fn rgba(w: u32, h: u32, pixels: &[[u8; 4]]) -> RgbaImage {
        RgbaImage::from_raw(w, h, pixels.iter().flatten().copied().collect())
            .expect("pixel buffer matches dimensions")
    }

    #[test]
    fn svg_merges_same_colour_runs_into_paths() {
        // 3x2: a red 2x2 block beside a full-height blue column.
        let img = rgba(
            3,
            2,
            &[
                [255, 0, 0, 255],
                [255, 0, 0, 255],
                [0, 0, 255, 255],
                [255, 0, 0, 255],
                [255, 0, 0, 255],
                [0, 0, 255, 255],
            ],
        );
        let svg = encode_svg(&img);
        assert!(
            svg.contains("width=\"3\" height=\"2\" viewBox=\"0 0 3 2\""),
            "{svg}"
        );
        // One path per colour, whichever order the RGBA key sorts in.
        assert_eq!(svg.matches("<path ").count(), 2, "{svg}");
        assert!(
            svg.contains("<path fill=\"#0000ff\" d=\"M2 0h1v2h-1z\"/>"),
            "{svg}"
        );
        assert!(
            svg.contains("<path fill=\"#ff0000\" d=\"M0 0h2v2h-2z\"/>"),
            "{svg}"
        );
        assert!(svg.ends_with("</svg>"));
    }

    #[test]
    fn svg_omits_transparent_pixels_and_preserves_alpha() {
        // Transparent black beside half-transparent green, one pixel each.
        let img = rgba(2, 1, &[[0, 0, 0, 0], [0, 128, 0, 128]]);
        let svg = encode_svg(&img);
        assert_eq!(svg.matches("<path ").count(), 1, "{svg}");
        assert!(
            svg.contains("<path fill=\"#008000\" fill-opacity=\"0.502\" d=\"M1 0h1v1h-1z\"/>"),
            "{svg}"
        );
    }
}
