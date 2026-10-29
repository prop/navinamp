//! Integration tests for the classic Winamp 2 (`.wsz`) synthesis half of
//! `navinamp-skin`.
//!
//! All fixtures are synthetic `.wsz` archives built in-test with the `zip`
//! writer; the BMPs inside them are generated with the `image` crate at the
//! exact dimensions of the real classic spec (verified against the Winamp
//! 2.91 base skin) — no binary fixtures, no downloads, no copyrighted
//! assets.

use std::io::Write;

use image::{DynamicImage, ImageFormat, RgbImage};

use navinamp_skin::{
    GuiObject, Layout, Skin, SkinArchive, SkinError, SliderAction, Warning, bitmap_payloads,
    is_classic,
};

/// Builds a `.wsz`/`.wal` (ZIP) from name/content pairs.
fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let cursor = std::io::Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    let options = zip::write::SimpleFileOptions::default();
    for (name, data) in entries {
        writer.start_file(*name, options).expect("start_file");
        writer.write_all(data).expect("write entry");
    }
    writer.finish().expect("finish zip").into_inner()
}

/// A solid 24-bit BMP of the given size (classic skins' native format).
fn bmp(w: u32, h: u32) -> Vec<u8> {
    let image = RgbImage::from_pixel(w, h, image::Rgb([90, 12, 200]));
    let mut buf = Vec::new();
    DynamicImage::ImageRgb8(image)
        .write_to(&mut std::io::Cursor::new(&mut buf), ImageFormat::Bmp)
        .expect("encode fixture bmp");
    buf
}

/// The 24 viscolor.txt lines of the classic base skin (values only matter
/// here, not the exact base colors).
const VISCOLOR_TXT: &str = "\
0,0,0, // color 0 = black
24,33,41, // color 1 = grey for dots
239,49,16, // color 2 = top of spec
206,41,16, // 3
214,90,0, // 4
214,102,0, // 5
214,115,0, // 6
198,123,8, // 7
222,165,24, // 8
214,181,33, // 9
189,222,41, // 10
148,222,33, // 11
41,206,16, // 12
50,190,16, // 13
57,181,16, // 14
49,156,8,  // 15
41,148,0,  // 16
24,132,8,   // 17 = bottom of spec
255,255,255, // 18 = osc 1
214,214,222, // 19 = osc 2
181,189,189, // 20 = osc 3
160,170,175,  // 21 = osc 4
148,156,165,  // 22 = osc 4
150, 150, 150, // 23 = analyzer peak dots
";

/// pledit.txt in the modern INI dialect (base 2.9x form).
const PLEDIT_INI_TXT: &str = "\
[Text]
Normal=#00FF00
Current=#FFFFFF
NormalBG=#000000
SelectedBG=#0000C6
Font=Arial
";

/// Every classic-spec BMP at its true base-skin dimensions.
const FULL_BMPS: &[(&str, u32, u32)] = &[
    ("main.bmp", 275, 116),
    ("titlebar.bmp", 344, 87),
    ("cbuttons.bmp", 136, 36),
    ("posbar.bmp", 307, 10),
    ("volume.bmp", 68, 433),
    ("balance.bmp", 68, 433),
    ("shufrep.bmp", 92, 85),
    ("text.bmp", 155, 74),
    ("numbers.bmp", 99, 13),
    ("playpaus.bmp", 42, 9),
    ("monoster.bmp", 58, 24),
    ("eqmain.bmp", 275, 315),
    ("eq_ex.bmp", 275, 82),
    ("pledit.bmp", 280, 186),
];

/// A complete classic skin: every optional file present, both text files in
/// their modern dialects.
fn full_classic_skin() -> Vec<u8> {
    let mut entries: Vec<(&str, Vec<u8>)> = Vec::new();
    for (name, w, h) in FULL_BMPS {
        entries.push((name, bmp(*w, *h)));
    }
    entries.push(("viscolor.txt", VISCOLOR_TXT.as_bytes().to_vec()));
    entries.push(("pledit.txt", PLEDIT_INI_TXT.as_bytes().to_vec()));
    let borrowed: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(name, data)| (*name, data.as_slice()))
        .collect();
    zip_bytes(&borrowed)
}

/// Parses a synthetic classic skin, asserting it was actually synthesized
/// (not routed to the XML path).
fn parse_classic(bytes: Vec<u8>) -> (SkinArchive, Skin, Vec<Warning>) {
    let archive = SkinArchive::from_bytes(bytes).expect("archive");
    assert!(is_classic(&archive), "fixture must be detected as classic");
    let (skin, warnings) = Skin::from_archive(&archive).expect("synthesize");
    (archive, skin, warnings)
}

/// The `normal` layout of one container.
fn normal_layout<'a>(skin: &'a Skin, container: &str) -> &'a Layout {
    skin.containers
        .iter()
        .find(|c| c.id == container)
        .unwrap_or_else(|| panic!("container `{container}`"))
        .layouts
        .iter()
        .find(|l| l.id == "normal")
        .unwrap_or_else(|| panic!("layout `normal` of `{container}`"))
}

/// All objects of one Wasabi tag kind inside a layout.
fn of_kind<'a>(layout: &'a Layout, tag: &str) -> Vec<&'a GuiObject> {
    layout
        .objects
        .iter()
        .filter(|o| o.tag_name() == tag)
        .collect()
}

/// The `button`/`togglebutton` with the given action, if any.
fn with_action<'a>(layout: &'a Layout, action: &str) -> Option<&'a GuiObject> {
    layout.objects.iter().find(|o| match o {
        GuiObject::Button(b) => b.action.as_deref() == Some(action),
        GuiObject::ToggleButton(t) => t.action.as_deref() == Some(action),
        GuiObject::Slider(s) => s.action.as_deref() == Some(action),
        _ => false,
    })
}

// 1) detection -----------------------------------------------------------------

#[test]
fn classic_detection_distinguishes_wsz_from_wal_and_junk() {
    // classic: no skin.xml + several hallmarks
    let archive = SkinArchive::from_bytes(full_classic_skin()).unwrap();
    assert!(is_classic(&archive));

    // .wal: skin.xml present → never classic, XML path unaffected
    let wal = zip_bytes(&[("skin.xml", b"<skin/>".as_slice())]);
    let archive = SkinArchive::from_bytes(wal).unwrap();
    assert!(!is_classic(&archive));

    // only one hallmark → not enough (guards against random BMP zips)
    let one = zip_bytes(&[("main.bmp", bmp(275, 116).as_slice())]);
    let archive = SkinArchive::from_bytes(one).unwrap();
    assert!(!is_classic(&archive));
    assert!(matches!(
        Skin::from_archive(&archive),
        Err(SkinError::MissingSkinXml)
    ));

    // plain junk zip → still the classic MissingSkinXml error
    let junk = zip_bytes(&[("readme.txt", b"hello".as_slice())]);
    let archive = SkinArchive::from_bytes(junk).unwrap();
    assert!(!is_classic(&archive));
    assert!(matches!(
        Skin::from_archive(&archive),
        Err(SkinError::MissingSkinXml)
    ));
}

#[test]
fn classic_entries_are_found_case_insensitively_and_nested() {
    // some .wsz zips nest everything one folder deep with inconsistent casing
    let mut entries: Vec<(&str, Vec<u8>)> = vec![
        ("Skins/Base/MAIN.BMP", bmp(275, 116)),
        ("Skins/Base/CBUTTONS.BMP", bmp(136, 36)),
        ("Skins/Base/shufrep.bmp", bmp(92, 85)),
        ("Skins/Base/pledit.txt", PLEDIT_INI_TXT.as_bytes().to_vec()),
    ];
    // posbar etc. omitted on purpose: optional, only warned about
    entries.push(("Skins/Base/VISCOLOR.TXT", VISCOLOR_TXT.as_bytes().to_vec()));
    let borrowed: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(name, data)| (*name, data.as_slice()))
        .collect();
    let (_archive, skin, _warnings) = parse_classic(zip_bytes(&borrowed));

    // the background layer points at the actual (uppercase, nested) entry
    let main = normal_layout(&skin, "main");
    let bg = match main.objects.first() {
        Some(GuiObject::Layer(l)) => l,
        other => panic!("expected background layer, got {other:?}"),
    };
    assert_eq!(bg.image.as_deref(), Some("wsz.main"));
    let bitmap = skin
        .elements
        .bitmaps
        .iter()
        .find(|b| b.id == "wsz.main")
        .unwrap();
    assert_eq!(bitmap.file.as_deref(), Some("Skins/Base/MAIN.BMP"));
}

// 2) full model ------------------------------------------------------------------

#[test]
fn full_classic_skin_synthesizes_three_containers() {
    let (_archive, skin, warnings) = parse_classic(full_classic_skin());
    assert!(
        warnings.is_empty(),
        "complete fixture must synthesize warning-free: {warnings:?}"
    );

    assert_eq!(
        skin.params.get("format").map(String::as_str),
        Some("classic-wsz")
    );
    let ids: Vec<&str> = skin.containers.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, vec!["main", "eq", "playlist"]);

    let main = normal_layout(&skin, "main");
    assert_eq!((main.rect.w, main.rect.h), (Some(275), Some(116)));
    let eq = normal_layout(&skin, "eq");
    assert_eq!((eq.rect.w, eq.rect.h), (Some(275), Some(116)));
    let playlist = normal_layout(&skin, "playlist");
    assert_eq!((playlist.rect.w, playlist.rect.h), (Some(275), Some(232)));

    // object census (see the coordinate table in wsz.rs):
    // main: bg + clutterbar layers, 6 transports, 4 toggles, 3 sliders,
    // 4 texts (songticker/time/bitrate/khz), 3 statuses, 1 vis, 4 sysbuttons
    assert_eq!(of_kind(main, "layer").len(), 2);
    assert_eq!(of_kind(main, "button").len(), 6 + 4); // transport + sysbuttons
    assert_eq!(of_kind(main, "togglebutton").len(), 4);
    assert_eq!(of_kind(main, "slider").len(), 3);
    assert_eq!(of_kind(main, "text").len(), 4);
    assert_eq!(of_kind(main, "status").len(), 3);
    assert_eq!(of_kind(main, "vis").len(), 1);

    // eq: bg + titlebar + graph layers, on/auto toggles, presets button,
    // preamp + 10 band sliders
    assert_eq!(of_kind(eq, "layer").len(), 3);
    assert_eq!(of_kind(eq, "togglebutton").len(), 2);
    assert_eq!(of_kind(eq, "button").len(), 1);
    assert_eq!(of_kind(eq, "slider").len(), 11);

    // playlist: pledit.bmp tiles
    assert_eq!(of_kind(playlist, "layer").len(), 9);
}

#[test]
fn classic_skin_serde_round_trips() {
    let (_archive, skin, _warnings) = parse_classic(full_classic_skin());
    let json = skin.to_json().expect("to_json");
    let round: Skin = serde_json::from_str(&json).expect("from_json");
    assert_eq!(round, skin, "JSON round-trip must be lossless");
}

// 3) actions + image wiring --------------------------------------------------------

#[test]
fn transport_buttons_carry_actions_and_images() {
    let (_archive, skin, _warnings) = parse_classic(full_classic_skin());
    let main = normal_layout(&skin, "main");

    // (action, window-x, window-y, width)
    let expected = [
        ("PREV", 16, 88, 23),
        ("PLAY", 39, 88, 23),
        ("PAUSE", 62, 88, 23),
        ("STOP", 85, 88, 23),
        ("NEXT", 108, 88, 22),
        ("EJECT", 136, 89, 22),
    ];
    for (action, x, y, w) in expected {
        let button = match with_action(main, action) {
            Some(GuiObject::Button(b)) => b,
            other => panic!("expected `{action}` button, got {other:?}"),
        };
        assert_eq!(button.rect.x, Some(x), "`{action}` x");
        assert_eq!(button.rect.y, Some(y), "`{action}` y");
        assert_eq!(button.rect.w, Some(w), "`{action}` w");
        assert_eq!(button.rect.h, Some(16), "`{action}` h");
        // normal + down images exist as bitmaps, crops from cbuttons.bmp
        for id in [&button.image, &button.downimage] {
            let id = id.as_deref().expect("image id");
            let bitmap = skin
                .elements
                .bitmaps
                .iter()
                .find(|b| b.id == id)
                .unwrap_or_else(|| panic!("bitmap `{id}` for `{action}`"));
            assert_eq!(bitmap.file.as_deref(), Some("cbuttons.bmp"));
        }
    }
}

#[test]
fn sliders_carry_seek_volume_balance() {
    let (_archive, skin, _warnings) = parse_classic(full_classic_skin());
    let main = normal_layout(&skin, "main");

    let seek = match with_action(main, "SEEK") {
        Some(GuiObject::Slider(s)) => s,
        other => panic!("expected seek slider, got {other:?}"),
    };
    assert_eq!(seek.action_kind(), Some(SliderAction::Seek));
    assert_eq!((seek.rect.x, seek.rect.y), (Some(16), Some(72)));
    assert_eq!((seek.rect.w, seek.rect.h), (Some(248), Some(10)));
    assert_eq!(seek.thumbimage.as_deref(), Some("wsz.seek.thumb"));
    assert_eq!(
        seek.params.get("thumbdownimage").map(String::as_str),
        Some("wsz.seek.thumb.down")
    );

    let volume = match with_action(main, "VOLUME") {
        Some(GuiObject::Slider(s)) => s,
        other => panic!("expected volume slider, got {other:?}"),
    };
    assert_eq!(volume.action_kind(), Some(SliderAction::Volume));
    assert_eq!((volume.rect.x, volume.rect.y), (Some(107), Some(57)));
    assert_eq!((volume.rect.w, volume.rect.h), (Some(68), Some(13)));

    let balance = match with_action(main, "BALANCE") {
        Some(GuiObject::Slider(s)) => s,
        other => panic!("expected balance slider, got {other:?}"),
    };
    assert_eq!(balance.action_kind(), Some(SliderAction::Balance));
    assert_eq!((balance.rect.x, balance.rect.y), (Some(177), Some(57)));
    assert_eq!((balance.rect.w, balance.rect.h), (Some(38), Some(13)));
}

#[test]
fn toggles_and_eq_sliders_are_wired() {
    let (_archive, skin, _warnings) = parse_classic(full_classic_skin());
    let main = normal_layout(&skin, "main");
    for (action, x) in [
        ("TOGGLE_SHUFFLE", 164),
        ("TOGGLE_REPEAT", 210),
        ("TOGGLE_EQ", 219),
        ("TOGGLE_PLAYLIST", 242),
    ] {
        let toggle = match with_action(main, action) {
            Some(GuiObject::ToggleButton(t)) => t,
            other => panic!("expected `{action}` toggle, got {other:?}"),
        };
        assert_eq!(toggle.rect.x, Some(x), "`{action}` x");
        // three-state skins carry the active image in params
        let id = toggle.id.as_deref().unwrap().strip_prefix("wsz.").unwrap();
        let expected_active = format!("wsz.{id}.active");
        assert_eq!(
            toggle.params.get("activeimage").map(String::as_str),
            Some(expected_active.as_str()),
            "`{action}` active image"
        );
    }

    // EQ: preamp at index 0 + bands 1..=10 with matching frequencies and
    // x positions 21 / 78+18i
    let eq = normal_layout(&skin, "eq");
    let mut sliders = of_kind(eq, "slider")
        .iter()
        .filter_map(|o| match o {
            GuiObject::Slider(s) => Some(s),
            _ => None,
        })
        .collect::<Vec<_>>();
    sliders.sort_by_key(|s| s.rect.x);
    assert_eq!(sliders.len(), 11);
    for (i, slider) in sliders.iter().enumerate() {
        assert_eq!(slider.action.as_deref(), Some("EQ_SET"));
        assert_eq!(slider.params.get("index"), Some(&i.to_string()));
        assert_eq!(slider.rect.y, Some(38));
        assert_eq!((slider.rect.w, slider.rect.h), (Some(14), Some(63)));
    }
    assert_eq!(sliders[0].rect.x, Some(21));
    assert_eq!(
        sliders[0].params.get("band").map(String::as_str),
        Some("preamp")
    );
    for (i, slider) in sliders[1..].iter().enumerate() {
        assert_eq!(slider.rect.x, Some(78 + 18 * i as i32));
        let hz = [60, 170, 310, 600, 1000, 3000, 6000, 12000, 14000, 16000][i];
        assert_eq!(slider.params.get("band"), Some(&hz.to_string()));
    }
}

// 4) missing optionals -----------------------------------------------------------

#[test]
fn missing_optional_files_skip_objects_but_parse() {
    // every optional BMP family removed: a genuinely minimal skin
    let entries: Vec<(&str, Vec<u8>)> = vec![
        ("main.bmp", bmp(275, 116)),
        ("cbuttons.bmp", bmp(136, 36)),
        ("pledit.txt", PLEDIT_INI_TXT.as_bytes().to_vec()),
    ];
    let borrowed: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(name, data)| (*name, data.as_slice()))
        .collect();
    let (_archive, skin, warnings) = parse_classic(zip_bytes(&borrowed));

    // parse succeeded with the main window intact
    let main = normal_layout(&skin, "main");
    assert_eq!(of_kind(main, "button").len(), 6, "transport buttons only");
    assert_eq!(
        of_kind(main, "slider").len(),
        0,
        "posbar/volume/balance absent"
    );

    // exactly one warning per missing file (not per dropped object)
    let mut missing: Vec<&str> = warnings
        .iter()
        .filter_map(|w| match w {
            Warning::ClassicMissingOptional { file, .. } => Some(file.as_str()),
            _ => None,
        })
        .collect();
    missing.sort_unstable();
    missing.dedup();
    assert_eq!(
        missing,
        vec![
            "balance.bmp",
            "eqmain.bmp",
            "monoster.bmp",
            "numbers.bmp",
            "playpaus.bmp",
            "pledit.bmp",
            "posbar.bmp",
            "shufrep.bmp",
            "text.bmp",
            "titlebar.bmp",
            "volume.bmp",
        ],
        "one warning per missing file: {warnings:#?}"
    );

    // the eq container exists but is empty (eqmain.bmp drives everything)
    let eq = normal_layout(&skin, "eq");
    assert!(
        eq.objects.is_empty(),
        "eq objects skipped with eqmain absent"
    );
}

// 5) pledit.txt dialects ----------------------------------------------------------

#[test]
fn pledit_ini_colors_land_in_params() {
    let (_archive, skin, _warnings) = parse_classic(full_classic_skin());
    assert_eq!(
        skin.params.get("pledit.color.normal").map(String::as_str),
        Some("#00ff00")
    );
    assert_eq!(
        skin.params.get("pledit.color.current").map(String::as_str),
        Some("#ffffff")
    );
    assert_eq!(
        skin.params.get("pledit.color.normalbg").map(String::as_str),
        Some("#000000")
    );
    assert_eq!(
        skin.params
            .get("pledit.color.selectedbg")
            .map(String::as_str),
        Some("#0000c6")
    );
    assert_eq!(
        skin.params.get("pledit.font").map(String::as_str),
        Some("Arial")
    );
    assert_eq!(
        skin.params.get("pledit.colors").map(String::as_str),
        Some("normal=#00ff00,current=#ffffff,normalbg=#000000,selectedbg=#0000c6")
    );
}

#[test]
fn pledit_ancient_dialect_with_junk_header_parses() {
    let entries: Vec<(&str, Vec<u8>)> = vec![
        ("main.bmp", bmp(275, 116)),
        ("cbuttons.bmp", bmp(136, 36)),
        // junk header line, then bare R,G,B lines
        (
            "pledit.txt",
            b"Winamp Playlist Editor colors??\n0,255,0\n0,0,0\n255,255,255\n".to_vec(),
        ),
    ];
    let borrowed: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(name, data)| (*name, data.as_slice()))
        .collect();
    let (_archive, skin, _warnings) = parse_classic(zip_bytes(&borrowed));

    assert_eq!(
        skin.params.get("pledit.color.normal").map(String::as_str),
        Some("#00ff00")
    );
    assert_eq!(
        skin.params.get("pledit.color.normalbg").map(String::as_str),
        Some("#000000")
    );
    assert_eq!(
        skin.params.get("pledit.color.current").map(String::as_str),
        Some("#ffffff")
    );
}

// 6) viscolor blob ------------------------------------------------------------------

#[test]
fn viscolor_blob_carries_24_entries() {
    let (_archive, skin, _warnings) = parse_classic(full_classic_skin());
    let main = normal_layout(&skin, "main");
    let vis_objects = of_kind(main, "vis");
    let vis = vis_objects.first().expect("vis object");
    let blob = match vis {
        GuiObject::Vis(v) => v.params.get("viscolors").expect("viscolors param"),
        other => panic!("expected vis, got {other:?}"),
    };
    let entries: Vec<&str> = blob.split(';').collect();
    assert_eq!(entries.len(), 24);
    assert_eq!(entries[0], "0,0,0");
    assert_eq!(entries[1], "24,33,41");
    assert_eq!(entries[23], "150,150,150");
}

// 7) .wal regression -----------------------------------------------------------------

#[test]
fn wal_skins_never_route_to_the_synthesizer() {
    let xml = r#"
<skin>
  <elements><bitmap id="bg" file="bg.png"/></elements>
  <container id="main"><layout id="normal" w="100" h="50"/></container>
</skin>"#;
    let wal = zip_bytes(&[("skin.xml", xml.as_bytes()), ("bg.png", b"\x89PNG-fake")]);
    let archive = SkinArchive::from_bytes(wal).unwrap();
    assert!(!is_classic(&archive));

    let (skin, warnings) = Skin::from_archive(&archive).expect("wal parses via the XML path");
    // model comes from the XML, untouched by classic synthesis
    assert_eq!(skin.params.get("format"), None);
    assert_eq!(skin.containers[0].layouts[0].rect.w, Some(100));
    assert_eq!(skin.elements.bitmaps.len(), 1);
    assert!(
        !warnings
            .iter()
            .any(|w| matches!(w, Warning::ClassicMissingOptional { .. })),
        ".wal behavior must stay unchanged: {warnings:?}"
    );
}

// bonus: the synthesized bitmaps flow through the payload pipeline ----------

#[test]
fn classic_bitmaps_materialize_into_payloads() {
    let (archive, skin, warnings) = parse_classic(full_classic_skin());
    assert!(warnings.is_empty());
    let (images, payload_warnings) = bitmap_payloads(&archive, &skin);
    assert!(
        payload_warnings.is_empty(),
        "every documented crop must fit the fixture BMPs: {payload_warnings:?}"
    );
    // one payload per emitted bitmap + bitmapfont file… fonts aren't
    // materialized; bitmaps are
    assert_eq!(images.len(), skin.elements.bitmaps.len());
    assert!(
        images
            .values()
            .all(|url| url.starts_with("data:image/png;base64,"))
    );
    // and the transports' crops decode at button size
    let decode = |id: &str| -> (u32, u32) {
        let url = &images[id];
        let bytes = base64_decode(url.strip_prefix("data:image/png;base64,").unwrap());
        image::load_from_memory(&bytes)
            .unwrap()
            .to_rgba8()
            .dimensions()
    };
    assert_eq!(decode("wsz.btn.prev"), (23, 16));
    assert_eq!(decode("wsz.btn.next"), (22, 16));
    assert_eq!(decode("wsz.main"), (275, 116));
    assert_eq!(decode("wsz.vol.thumb"), (14, 11));
}

fn base64_decode(data: &str) -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .expect("base64")
}
