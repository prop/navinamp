//! Integration tests for the bitmap-payload half of `navamp-skin`.
//!
//! All fixtures are synthetic `.wal` archives built in-test with the `zip`
//! writer; the images inside them are generated with the `image` crate —
//! no binary fixtures, no downloads, no copyrighted assets.

use std::collections::HashMap;
use std::io::Write;

use base64::Engine as _;
use image::{DynamicImage, ImageFormat, RgbImage, Rgba, RgbaImage};

use navamp_skin::{Skin, SkinArchive, Warning, bitmap_payloads};

/// Builds a `.wal` (ZIP) from name/content pairs.
fn wal(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let cursor = std::io::Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    let options = zip::write::SimpleFileOptions::default();
    for (name, data) in entries {
        writer.start_file(*name, options).expect("start_file");
        writer.write_all(data).expect("write entry");
    }
    writer.finish().expect("finish zip").into_inner()
}

/// Parses `entries` into a skin and materializes its bitmap payloads.
fn payloads(entries: &[(&str, &[u8])]) -> (HashMap<String, String>, Vec<Warning>) {
    let archive = SkinArchive::from_bytes(wal(entries)).expect("archive");
    let (skin, _) = Skin::from_archive(&archive).expect("parse");
    bitmap_payloads(&archive, &skin)
}

/// A pixel color unique per (x, y), so crops are easy to verify.
fn xy_color(x: u32, y: u32) -> Rgba<u8> {
    Rgba([16 + x as u8 * 5, 32 + y as u8 * 7, 200, 255])
}

/// Encodes a synthetic RGBA PNG.
fn png_bytes(w: u32, h: u32, color: impl Fn(u32, u32) -> Rgba<u8>) -> Vec<u8> {
    encode(
        DynamicImage::ImageRgba8(RgbaImage::from_fn(w, h, color)),
        ImageFormat::Png,
    )
}

/// Encodes a synthetic 24-bit RGB BMP (the classic color-keyed format).
fn bmp_bytes(w: u32, h: u32, color: impl Fn(u32, u32) -> Rgba<u8>) -> Vec<u8> {
    let rgb = RgbImage::from_fn(w, h, |x, y| {
        let c = color(x, y).0;
        image::Rgb([c[0], c[1], c[2]])
    });
    encode(DynamicImage::ImageRgb8(rgb), ImageFormat::Bmp)
}

fn encode(image: DynamicImage, format: ImageFormat) -> Vec<u8> {
    let mut buf = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut buf), format)
        .expect("encode fixture");
    buf
}

/// Decodes a `data:image/png;base64,...` payload back into pixels.
fn decode_payload(url: &str) -> RgbaImage {
    let b64 = url
        .strip_prefix("data:image/png;base64,")
        .expect("data-url prefix");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .expect("base64");
    image::load_from_memory(&bytes)
        .expect("valid png payload")
        .to_rgba8()
}

// 1) whole-image payloads ------------------------------------------------------

#[test]
fn whole_image_payload_matches_source_pixels() {
    let xml = r#"<skin><elements><bitmap id="bg" file="bg.png"/></elements></skin>"#;
    let (images, warnings) = payloads(&[
        ("skin.xml", xml.as_bytes()),
        ("bg.png", &png_bytes(4, 3, xy_color)),
    ]);
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    assert_eq!(images.len(), 1);
    assert!(images["bg"].starts_with("data:image/png;base64,"));

    let rgba = decode_payload(&images["bg"]);
    assert_eq!(rgba.dimensions(), (4, 3));
    // sample a few pixels against the source
    assert_eq!(rgba.get_pixel(0, 0), &xy_color(0, 0));
    assert_eq!(rgba.get_pixel(3, 2), &xy_color(3, 2));
    assert_eq!(rgba.get_pixel(2, 1), &xy_color(2, 1));
}

// 2) crop correctness ----------------------------------------------------------

#[test]
fn crop_payload_samples_the_sub_rect() {
    let xml = r#"
<skin>
  <elements>
    <bitmap id="tile" file="Tile.PNG" x="1" y="1" w="2" h="2"/>
    <bitmap id="rest" file="Tile.PNG" x="2"/>
  </elements>
</skin>"#;
    // note: file lookup is case-insensitive, entry stored as `tile.png`
    let (images, warnings) = payloads(&[
        ("skin.xml", xml.as_bytes()),
        ("tile.png", &png_bytes(4, 4, xy_color)),
    ]);
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

    // (1,1,2,2) crop keeps exactly the source sub-rect
    let tile = decode_payload(&images["tile"]);
    assert_eq!(tile.dimensions(), (2, 2));
    assert_eq!(tile.get_pixel(0, 0), &xy_color(1, 1));
    assert_eq!(tile.get_pixel(1, 0), &xy_color(2, 1));
    assert_eq!(tile.get_pixel(0, 1), &xy_color(1, 2));
    assert_eq!(tile.get_pixel(1, 1), &xy_color(2, 2));

    // x-only crop runs to the right edge of the source
    let rest = decode_payload(&images["rest"]);
    assert_eq!(rest.dimensions(), (2, 4));
    assert_eq!(rest.get_pixel(0, 3), &xy_color(2, 3));
    assert_eq!(rest.get_pixel(1, 3), &xy_color(3, 3));
}

// 3) Wasabi color-key transparency ---------------------------------------------

#[test]
fn transparent_magic_pink_bmp_gains_alpha() {
    let pink = Rgba([255, 0, 255, 255]);
    let black = Rgba([0, 0, 0, 255]);
    let color = |x, _| if x % 2 == 0 { pink } else { black };
    let xml = r##"
<skin>
  <elements>
    <bitmap id="bmp.pink" file="btn.bmp" transparent="255,0,255"/>
    <bitmap id="hex.pink" file="btn.bmp" transparent="#FF00FF"/>
    <bitmap id="bare.pink" file="btn2.png" transparent="FF00FF"/>
  </elements>
</skin>"##;
    let (images, warnings) = payloads(&[
        ("skin.xml", xml.as_bytes()),
        ("btn.bmp", &bmp_bytes(3, 1, color)),
        ("btn2.png", &png_bytes(3, 1, color)),
    ]);
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

    for id in ["bmp.pink", "hex.pink", "bare.pink"] {
        let rgba = decode_payload(&images[id]);
        for x in 0..3 {
            let pixel = rgba.get_pixel(x, 0);
            let expected_alpha = if x % 2 == 0 { 0 } else { 255 };
            assert_eq!(pixel.0[3], expected_alpha, "alpha at ({x},0) of {id}");
            // color bytes are kept, only the alpha changes
            assert_eq!(
                *pixel,
                with_alpha(color(x, 0), expected_alpha),
                "rgb at ({x},0) of {id}"
            );
        }
    }
}

/// `color` with its alpha replaced (the color-key contract).
fn with_alpha(color: Rgba<u8>, alpha: u8) -> Rgba<u8> {
    Rgba([color.0[0], color.0[1], color.0[2], alpha])
}

#[test]
fn unparseable_transparent_value_is_noop_warning() {
    let xml = r#"
<skin>
  <elements>
    <bitmap id="bg" file="bg.png" transparent="pretty in pink"/>
  </elements>
</skin>"#;
    let (images, warnings) = payloads(&[
        ("skin.xml", xml.as_bytes()),
        ("bg.png", &png_bytes(2, 2, xy_color)),
    ]);
    // the payload is still produced, fully opaque
    let rgba = decode_payload(&images["bg"]);
    assert_eq!(rgba.get_pixel(1, 1), &xy_color(1, 1));
    assert_eq!(
        warnings,
        vec![Warning::MalformedValue {
            element: "bitmap".into(),
            attribute: "transparent".into(),
            value: "pretty in pink".into(),
        }]
    );
}

// 4) missing file ---------------------------------------------------------------

#[test]
fn missing_bitmap_file_warns_and_skips_payload() {
    let xml = r#"
<skin>
  <elements>
    <bitmap id="bg" file="gone.png"/>
    <bitmap id="ok" file="ok.png"/>
  </elements>
</skin>"#;
    let (images, warnings) = payloads(&[
        ("skin.xml", xml.as_bytes()),
        ("ok.png", &png_bytes(1, 1, xy_color)),
    ]);
    assert_eq!(images.len(), 1, "only `ok` gets a payload: {images:?}");
    assert_eq!(
        warnings,
        vec![Warning::MissingFile {
            file: "gone.png".into(),
            referenced_by: "bitmap `bg`".into(),
        }]
    );
}

// 5) undecodable bytes -----------------------------------------------------------

#[test]
fn undecodable_file_warns_and_skips_payload() {
    let xml = r#"<skin><elements><bitmap id="bg" file="bg.png"/></elements></skin>"#;
    let (images, warnings) = payloads(&[
        ("skin.xml", xml.as_bytes()),
        ("bg.png", b"\x89PNG-not-really-a-png"),
    ]);
    assert!(images.is_empty());
    assert!(
        matches!(&warnings[..], [Warning::ImageDecode { file, .. }] if file == "bg.png"),
        "unexpected warnings: {warnings:?}"
    );
}

// 6) out-of-range crop ------------------------------------------------------------

#[test]
fn out_of_range_crop_warns_and_skips_payload() {
    let xml = r#"
<skin>
  <elements>
    <bitmap id="past.edge" file="bg.png" x="3" y="0" w="2" h="4"/>
    <bitmap id="zero.size" file="bg.png" x="0" y="0" w="0" h="2"/>
    <bitmap id="ok" file="bg.png" x="0" y="0" w="4" h="4"/>
  </elements>
</skin>"#;
    let (images, warnings) = payloads(&[
        ("skin.xml", xml.as_bytes()),
        ("bg.png", &png_bytes(4, 4, xy_color)),
    ]);
    assert_eq!(
        images.len(),
        1,
        "only the fitting rect gets a payload: {images:?}"
    );
    assert_eq!(
        warnings,
        vec![
            Warning::CropOutOfRange {
                bitmap: "past.edge".into(),
                file: "bg.png".into(),
                x: 3,
                y: 0,
                w: 2,
                h: 4,
                image_width: 4,
                image_height: 4,
            },
            Warning::CropOutOfRange {
                bitmap: "zero.size".into(),
                file: "bg.png".into(),
                x: 0,
                y: 0,
                w: 0,
                h: 2,
                image_width: 4,
                image_height: 4,
            },
        ]
    );
}

// 7) duplicate ids ------------------------------------------------------------------

#[test]
fn duplicate_bitmap_ids_keep_first_payload() {
    let xml = r#"
<skin>
  <elements>
    <bitmap id="dup" file="first.png"/>
    <bitmap id="dup" file="second.png"/>
  </elements>
</skin>"#;
    let (images, _warnings) = payloads(&[
        ("skin.xml", xml.as_bytes()),
        ("first.png", &png_bytes(2, 1, xy_color)),
        ("second.png", &png_bytes(8, 8, xy_color)),
    ]);
    // parse-time already warned DuplicateId; the payload map keeps `first`
    assert_eq!(images.len(), 1);
    assert_eq!(decode_payload(&images["dup"]).dimensions(), (2, 1));
}

// 8) bitmaps without a file are skipped silently -------------------------------------

#[test]
fn bitmap_without_file_is_skipped_silently() {
    let xml = r#"<skin><elements><bitmap id="ghost"/></elements></skin>"#;
    let (images, warnings) = payloads(&[("skin.xml", xml.as_bytes())]);
    assert!(images.is_empty());
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
}
