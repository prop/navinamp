//! Bitmap payload extraction: `<bitmap>` elements → PNG data-URLs.
//!
//! The parsing half ([`Skin`](crate::Skin)) keeps `elements.bitmaps` as
//! *references* to files inside the archive; this module materializes them:
//! every bitmap with a `file` is read (case-insensitively), decoded as
//! PNG/BMP, cropped to its `x`/`y`/`w`/`h` sub-rectangle when given, given
//! Wasabi color-key transparency when it carries a `transparent` attribute,
//! and re-encoded as a PNG data-URL the webview can drop into an `<img>`.
//!
//! Like the rest of the parser nothing here is fatal: a missing file,
//! undecodable bytes, an out-of-range crop or an unparseable transparency
//! value only costs a [`Warning`] — that bitmap simply gets no payload and
//! the renderer skips it.
//!
//! v1 deliberately does **no** downscaling on load: payloads are 1:1 with
//! the skin's own pixels and integer scaling (1×/2× for retina) is the
//! renderer's job (plan §3).

use std::collections::HashMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use image::RgbaImage;

use crate::archive::SkinArchive;
use crate::error::Warning;
use crate::model::{Params, Skin};

/// Prefix of every payload this module produces.
const DATA_URL_PREFIX: &str = "data:image/png;base64,";

/// Materializes every `<bitmap>` that references a file into a PNG data-URL.
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
/// # use navamp_skin::SkinArchive;
/// let archive = SkinArchive::open(Path::new("skin.wal"))?;
/// let (skin, _) = navamp_skin::Skin::from_archive(&archive)?;
/// let (images, warnings) = navamp_skin::bitmap_payloads(&archive, &skin);
/// // images: "player.background" -> "data:image/png;base64,iVBORw0..."
/// # Ok::<(), navamp_skin::SkinError>(())
/// ```
pub fn bitmap_payloads(
    archive: &SkinArchive,
    skin: &Skin,
) -> (HashMap<String, String>, Vec<Warning>) {
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

        match encode_png(rgba) {
            Ok(png) => {
                let url = format!("{DATA_URL_PREFIX}{}", STANDARD.encode(&png));
                images.insert(bitmap.id.clone(), url);
            }
            // Re-encoding an RGBA8 buffer as PNG cannot realistically fail;
            // treat it like a decode problem rather than inventing a third
            // variant for the impossible case.
            Err(err) => warnings.push(Warning::ImageDecode {
                file: file.to_owned(),
                message: format!("png re-encode failed: {err}"),
            }),
        }
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

/// Encodes an RGBA buffer as PNG bytes.
fn encode_png(rgba: RgbaImage) -> Result<Vec<u8>, image::ImageError> {
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(rgba)
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
}
