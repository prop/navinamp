//! The built-in fallback skin (`assets/fallback-skin`) must parse clean
//! through the whole pipeline: zero warnings from the parser, the expected
//! window metrics, and every bitmap materializing into a PNG payload.
//!
//! If this fails after editing the generator, run
//! `node assets/fallback-skin/build.mjs` and commit the regenerated
//! `out/fallback.wal`.

use std::path::Path;

use navinamp_skin::{Skin, SkinArchive, bitmap_payloads};

const WAL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/fallback-skin/out/fallback.wal"
);

fn open() -> (SkinArchive, Skin) {
    let archive = SkinArchive::open(Path::new(WAL))
        .expect("open fallback.wal — run `node assets/fallback-skin/build.mjs`");
    let (skin, warnings) = Skin::from_archive(&archive).expect("parse fallback skin");
    let rendered = warnings.iter().map(|w| w.to_string()).collect::<Vec<_>>();
    assert!(
        rendered.is_empty(),
        "fallback skin must parse warning-free:\n  {}",
        rendered.join("\n  ")
    );
    (archive, skin)
}

#[test]
fn fallback_skin_has_expected_windows() {
    let (_archive, skin) = open();

    let player = skin
        .containers
        .iter()
        .find(|c| c.id == "main")
        .expect("main container");
    let normal = player
        .layouts
        .iter()
        .find(|l| l.id == "normal")
        .expect("player normal layout");
    assert_eq!(normal.rect.w, Some(275));
    assert_eq!(normal.rect.h, Some(280));

    let playlist = skin
        .containers
        .iter()
        .find(|c| c.id == "playlist")
        .expect("playlist container");
    let pe = playlist
        .layouts
        .iter()
        .find(|l| l.id == "normal")
        .expect("playlist normal layout");
    assert_eq!(pe.rect.w, Some(275));
    assert_eq!(pe.rect.h, Some(232));
}

#[test]
fn fallback_skin_bitmaps_all_materialize() {
    let (archive, skin) = open();
    let with_file = skin
        .elements
        .bitmaps
        .iter()
        .filter(|b| b.file.is_some())
        .count();
    assert!(with_file > 0, "fallback skin defines bitmaps");

    let (images, warnings) = bitmap_payloads(&archive, &skin);
    let rendered = warnings.iter().map(|w| w.to_string()).collect::<Vec<_>>();
    assert!(
        rendered.is_empty(),
        "fallback skin bitmaps must materialize warning-free:\n  {}",
        rendered.join("\n  ")
    );
    assert_eq!(
        images.len(),
        with_file,
        "every file-backed bitmap gets a payload"
    );
    assert!(
        images
            .values()
            .all(|v| v.starts_with("data:image/png;base64,")),
        "payloads are PNG data URLs"
    );
}
