//! Integration tests for `navinamp-skin`.
//!
//! All fixtures are synthetic `.wal` archives built in-test with the `zip`
//! writer — no binary fixtures, no downloads, no copyrighted assets.

use std::io::Write;
use std::path::PathBuf;

use navinamp_skin::{GuiObject, Skin, SkinArchive, SkinError, SliderAction, Warning};

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

fn parse_bytes(entries: &[(&str, &[u8])]) -> (Skin, Vec<Warning>) {
    let archive = SkinArchive::from_bytes(wal(entries)).expect("archive");
    Skin::from_archive(&archive).expect("parse")
}

fn parse_skin_xml(xml: &str) -> (Skin, Vec<Warning>) {
    parse_bytes(&[("skin.xml", xml.as_bytes())])
}

/// The reference minimal skin used by several tests. Every referenced file
/// and bitmap id exists, so the only expected warnings are the two
/// intentionally-unknown tags.
const FULL_SKIN_XML: &str = r#"
<skin version="1.2">
  <elements>
    <bitmap id="player.background" file="player-bg.png" x="0" y="0" w="275" h="116" gammagroup="Display"/>
    <bitmap id="logo" file="logo.png"/>
    <bitmap id="button.play" file="button.play.png"/>
    <bitmap id="button.play.down" file="button.play.down.png"/>
    <bitmap id="button.play.hover" file="button.play.hover.png"/>
    <bitmap id="seek.thumb" file="seek.thumb.png"/>
    <bitmapfont id="tickerfont" file="fonts/ticker.png" charwidth="5" charheight="6"/>
    <truetypefont id="tahoma" file="fonts/tahoma.ttf" bold="0"/>
    <color id="player.display.color" value="255,255,255,255" unknownattr="keepme"/>
    <gamma id="green" amount="0.5"/>
    <mysteryelement custom="1"/>
  </elements>
  <groupdef id="player.buttons">
    <button id="btn.play" action="PLAY" image="button.play" downimage="button.play.down"
            hoverimage="button.play.hover" tooltip="Play" x="12" y="30"
            cfg="wasabi.play.cfg" ghost="0"/>
    <fancywidget foo="1" bar="two"><childtag a="b"/></fancywidget>
  </groupdef>
  <groupdef id="player.normal" background="player.background" x="0" y="0" w="275" h="116"
            desktopalpha="1">
    <group id="player.buttons" x="0" y="100" notify="extra"/>
    <layer id="logo.layer" image="logo" x="200" y="4" alpha="0.75"/>
    <text id="ticker" display="songticker" x="20" y="90" w="200" h="14" fontsize="12"/>
    <slider id="seeker" action="SEEK" x="10" y="110" w="255" h="8" thumbimage="seek.thumb"/>
  </groupdef>
  <container id="main" name="Main Window">
    <layout id="normal" x="0" y="0" w="275" h="116" autoatstart="1" background="player.background">
      <group id="player.normal"/>
    </layout>
  </container>
</skin>
"#;

const FULL_SKIN_FILES: &[(&str, &[u8])] = &[
    ("player-bg.png", b"\x89PNG-fake"),
    ("logo.png", b"\x89PNG-fake"),
    ("button.play.png", b"\x89PNG-fake"),
    ("button.play.down.png", b"\x89PNG-fake"),
    ("button.play.hover.png", b"\x89PNG-fake"),
    ("seek.thumb.png", b"\x89PNG-fake"),
    ("fonts/ticker.png", b"\x89PNG-fake"),
    ("fonts/tahoma.ttf", b"fake-ttf"),
];

fn parse_full_skin() -> (Skin, Vec<Warning>) {
    let mut entries: Vec<(&str, &[u8])> = vec![("skin.xml", FULL_SKIN_XML.as_bytes())];
    entries.extend_from_slice(FULL_SKIN_FILES);
    parse_bytes(&entries)
}

// 1) minimal synthetic skin: full model incl. preserved unknowns ------------

#[test]
fn minimal_skin_parses_fully() {
    let (skin, warnings) = parse_full_skin();

    // root attrs preserved
    assert_eq!(skin.params.get("version").map(String::as_str), Some("1.2"));

    // elements
    let elements = &skin.elements;
    assert_eq!(elements.bitmaps.len(), 6);
    let bg = &elements.bitmaps[0];
    assert_eq!(bg.id, "player.background");
    assert_eq!(bg.file.as_deref(), Some("player-bg.png"));
    assert_eq!(bg.x, Some(0));
    assert_eq!(bg.w, Some(275));
    assert_eq!(
        bg.params.get("gammagroup").map(String::as_str),
        Some("Display")
    );
    let logo = &elements.bitmaps[1];
    assert_eq!(logo.id, "logo");
    // absent coordinates = whole image
    assert_eq!(logo.x, None);
    assert_eq!(logo.y, None);
    assert_eq!(logo.w, None);
    assert_eq!(logo.h, None);
    assert!(logo.is_whole_image());

    let ticker = &elements.bitmap_fonts[0];
    assert_eq!(ticker.id, "tickerfont");
    assert_eq!(ticker.charwidth, Some(5));
    assert_eq!(ticker.charheight, Some(6));
    let ttf = &elements.truetype_fonts[0];
    assert_eq!(ttf.id, "tahoma");
    assert_eq!(ttf.params.get("bold").map(String::as_str), Some("0"));

    let color = &elements.colors[0];
    assert_eq!(color.id, "player.display.color");
    assert_eq!(color.value.as_deref(), Some("255,255,255,255"));
    // unknown attribute on a known element survives
    assert_eq!(
        color.params.get("unknownattr").map(String::as_str),
        Some("keepme")
    );

    let gamma = &elements.gammas[0];
    assert_eq!(gamma.id.as_deref(), Some("green"));
    assert_eq!(gamma.params.get("amount").map(String::as_str), Some("0.5"));

    // unknown element tag preserved generically
    assert_eq!(elements.other.len(), 1);
    assert_eq!(elements.other[0].tag, "mysteryelement");
    assert_eq!(
        elements.other[0].params.get("custom").map(String::as_str),
        Some("1")
    );

    // nested groupdefs
    assert_eq!(skin.groups.len(), 2);
    let buttons = &skin.groups["player.buttons"];
    assert_eq!(buttons.objects.len(), 2);
    let button = match &buttons.objects[0] {
        GuiObject::Button(b) => b,
        other => panic!("expected button, got {other:?}"),
    };
    assert_eq!(button.id.as_deref(), Some("btn.play"));
    assert_eq!(button.action.as_deref(), Some("PLAY"));
    assert_eq!(button.tooltip.as_deref(), Some("Play"));
    assert_eq!(button.rect.x, Some(12));
    assert_eq!(button.rect.y, Some(30));
    // arbitrary extra attributes preserved on known objects
    assert_eq!(
        button.params.get("cfg").map(String::as_str),
        Some("wasabi.play.cfg")
    );
    assert_eq!(button.params.get("ghost").map(String::as_str), Some("0"));

    let normal = &skin.groups["player.normal"];
    assert_eq!(normal.background.as_deref(), Some("player.background"));
    assert_eq!(normal.rect.w, Some(275));
    assert_eq!(normal.rect.h, Some(116));
    assert_eq!(
        normal.params.get("desktopalpha").map(String::as_str),
        Some("1")
    );
    assert_eq!(normal.objects.len(), 4);

    // group instance inside a groupdef: stored as a reference
    let instance = match &normal.objects[0] {
        GuiObject::Group(g) => g,
        other => panic!("expected group instance, got {other:?}"),
    };
    assert_eq!(instance.id.as_deref(), Some("player.buttons"));
    assert_eq!(instance.rect.x, Some(0));
    assert_eq!(
        instance.params.get("notify").map(String::as_str),
        Some("extra")
    );

    // layer with alpha kept raw in params
    let layer = match &normal.objects[1] {
        GuiObject::Layer(l) => l,
        other => panic!("expected layer, got {other:?}"),
    };
    assert_eq!(layer.image.as_deref(), Some("logo"));
    assert_eq!(layer.rect.x, Some(200));
    assert_eq!(layer.params.get("alpha").map(String::as_str), Some("0.75"));

    // text object
    let text = match &normal.objects[2] {
        GuiObject::Text(t) => t,
        other => panic!("expected text, got {other:?}"),
    };
    assert_eq!(text.display.as_deref(), Some("songticker"));
    assert_eq!(text.rect.w, Some(200));
    assert_eq!(text.params.get("fontsize").map(String::as_str), Some("12"));

    // slider with typed action helper
    let slider = match &normal.objects[3] {
        GuiObject::Slider(s) => s,
        other => panic!("expected slider, got {other:?}"),
    };
    assert_eq!(slider.action_kind(), Some(SliderAction::Seek));
    assert_eq!(slider.thumbimage.as_deref(), Some("seek.thumb"));

    // container / layout; unknown object tag preserved with subtree
    assert_eq!(skin.containers.len(), 1);
    let container = &skin.containers[0];
    assert_eq!(container.id, "main");
    assert_eq!(container.name.as_deref(), Some("Main Window"));
    let layout = &container.layouts[0];
    assert_eq!(layout.id, "normal");
    assert_eq!(
        layout.params.get("autoatstart").map(String::as_str),
        Some("1")
    );
    assert_eq!(
        layout.params.get("background").map(String::as_str),
        Some("player.background")
    );
    assert!(
        matches!(&layout.objects[0], GuiObject::Group(g) if g.id.as_deref() == Some("player.normal"))
    );

    // exactly the two intentional unknown tags warned about
    assert_eq!(
        warnings,
        vec![
            Warning::UnknownTag {
                tag: "mysteryelement".into(),
                parent: "elements".into()
            },
            Warning::UnknownTag {
                tag: "fancywidget".into(),
                parent: "groupdef `player.buttons`".into()
            },
        ]
    );

    // the unknown object kept its attributes and children
    let other = match &skin.groups["player.buttons"].objects[1] {
        GuiObject::Other(node) => node,
        other => panic!("expected generic node, got {other:?}"),
    };
    assert_eq!(other.tag, "fancywidget");
    assert_eq!(other.params.get("foo").map(String::as_str), Some("1"));
    assert_eq!(other.params.get("bar").map(String::as_str), Some("two"));
    assert_eq!(other.children.len(), 1);
    assert_eq!(other.children[0].tag, "childtag");
    assert_eq!(
        other.children[0].params.get("a").map(String::as_str),
        Some("b")
    );
}

// 2) case-insensitive file lookup -------------------------------------------

#[test]
fn file_lookup_is_case_insensitive() {
    let xml = r#"<skin><elements><bitmap id="bg" file="Player-BG.png"/></elements></skin>"#;
    let bytes = wal(&[
        ("Skin.XML", xml.as_bytes()),
        ("IMAGES/Logo.PNG", b"\x89PNG-fake"),
        ("Player-BG.png", b"\x89PNG-fake"),
    ]);
    let archive = SkinArchive::from_bytes(bytes).expect("archive");

    // case-insensitive reads, original names preserved in the listing
    assert!(archive.contains("skin.xml"));
    assert!(archive.contains("images/logo.png"));
    assert_eq!(archive.read_file("SKIN.XML").unwrap(), xml.as_bytes());
    assert_eq!(
        archive.read_file("images/logo.png").unwrap(),
        b"\x89PNG-fake"
    );
    assert_eq!(archive.read_file("PLAYER-BG.PNG").unwrap(), b"\x89PNG-fake");
    assert_eq!(
        archive.file_names(),
        vec![
            "IMAGES/Logo.PNG".to_owned(),
            "Player-BG.png".to_owned(),
            "Skin.XML".to_owned(),
        ]
    );
    assert!(matches!(
        archive.read_file("nope.png"),
        Err(SkinError::FileNotFound(_))
    ));

    // the referenced bitmap resolves despite the different casing
    let (skin, warnings) = Skin::from_archive(&archive).unwrap();
    assert_eq!(
        skin.elements.bitmaps[0].file.as_deref(),
        Some("Player-BG.png")
    );
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
}

// 3) missing referenced bitmap file -> warning -------------------------------

#[test]
fn missing_bitmap_file_warns_but_parses() {
    let xml = r#"
<skin>
  <elements>
    <bitmap id="bg" file="missing.png"/>
    <bitmap id="ok" file="present.png"/>
  </elements>
</skin>"#;
    let (skin, warnings) = parse_bytes(&[
        ("skin.xml", xml.as_bytes()),
        ("present.png", b"\x89PNG-fake"),
    ]);
    // model still carries the element
    assert_eq!(
        skin.elements.bitmaps[0].file.as_deref(),
        Some("missing.png")
    );
    assert_eq!(
        warnings,
        vec![Warning::MissingFile {
            file: "missing.png".into(),
            referenced_by: "bitmap `bg`".into(),
        }]
    );
}

// 4) dangling group reference -> warning -------------------------------------

#[test]
fn dangling_group_reference_warns() {
    let xml = r#"
<skin>
  <container id="main">
    <layout id="normal" w="100" h="50">
      <group id="does.not.exist"/>
      <group id="wasabi.standardframe"/> <!-- system group: exempt -->
    </layout>
  </container>
</skin>"#;
    let (skin, warnings) = parse_skin_xml(xml);
    assert_eq!(skin.groups.len(), 0);
    let layout = &skin.containers[0].layouts[0];
    assert_eq!(layout.objects.len(), 2);
    assert_eq!(
        warnings,
        vec![Warning::MissingGroup {
            id: "does.not.exist".into(),
            referenced_by: "container `main` / layout `normal`".into(),
        }]
    );
}

// 5) skin.xml missing -> error ------------------------------------------------

#[test]
fn missing_skin_xml_is_fatal() {
    let bytes = wal(&[("other.xml", b"<skin/>".as_slice())]);
    let archive = SkinArchive::from_bytes(bytes).unwrap();
    assert!(matches!(
        Skin::from_archive(&archive),
        Err(SkinError::MissingSkinXml)
    ));
}

// 6) serde round-trip ----------------------------------------------------------

#[test]
fn serde_round_trip_preserves_model() {
    let (skin, _) = parse_full_skin();
    let json = skin.to_json().expect("to_json");
    let round: Skin = serde_json::from_str(&json).expect("from_json");
    assert_eq!(round, skin, "JSON round-trip must be lossless");
}

// 7) group cycles -> warning ---------------------------------------------------

#[test]
fn group_cycles_warn() {
    let xml = r#"
<skin>
  <groupdef id="a"><group id="b"/></groupdef>
  <groupdef id="b"><group id="a"/></groupdef>
  <groupdef id="c"><group id="c"/></groupdef>
</skin>"#;
    let (skin, warnings) = parse_skin_xml(xml);
    assert_eq!(skin.groups.len(), 3);
    assert_eq!(
        warnings,
        vec![
            Warning::GroupCycle {
                path: vec!["a".into(), "b".into(), "a".into()]
            },
            Warning::GroupCycle {
                path: vec!["c".into(), "c".into()]
            },
        ]
    );
}

// 8) includes -------------------------------------------------------------------

#[test]
fn includes_are_parsed_and_cycles_warn() {
    let skin_xml = r#"
<skin>
  <include file="player-elements.xml"/>
  <include file="loop.xml"/>
</skin>"#;
    let elements_xml = r#"
<skin>
  <groupdef id="from.include"><button id="inc" action="PLAY"/></groupdef>
</skin>"#;
    let loop_xml = r#"<skin><include file="skin.xml"/></skin>"#;
    let (skin, warnings) = parse_bytes(&[
        ("skin.xml", skin_xml.as_bytes()),
        ("player-elements.xml", elements_xml.as_bytes()),
        ("loop.xml", loop_xml.as_bytes()),
    ]);
    // the included groupdef landed in the model
    let group = &skin.groups["from.include"];
    assert!(
        matches!(&group.objects[0], GuiObject::Button(b) if b.action.as_deref() == Some("PLAY"))
    );
    assert_eq!(
        warnings,
        vec![Warning::IncludeCycle {
            file: "skin.xml".into()
        }]
    );
}

#[test]
fn missing_include_file_warns() {
    let xml = r#"<skin><include file="nope.xml"/></skin>"#;
    let (skin, warnings) = parse_skin_xml(xml);
    assert_eq!(skin.groups.len(), 0);
    assert_eq!(
        warnings,
        vec![Warning::MissingFile {
            file: "nope.xml".into(),
            referenced_by: "<include>".into(),
        }]
    );
}

// 9) opening from a path --------------------------------------------------------

#[test]
fn archive_opens_from_path() {
    let bytes = wal(&[
        ("skin.xml", "<skin/>".as_bytes()),
        ("a.png", b"\x89PNG-fake"),
    ]);
    let path: PathBuf =
        std::env::temp_dir().join(format!("navinamp-skin-test-{}.wal", std::process::id()));
    std::fs::write(&path, &bytes).unwrap();
    let archive = SkinArchive::open(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(archive.contains("a.png"));
    let (skin, warnings) = Skin::from_archive(&archive).unwrap();
    assert_eq!(skin.containers.len(), 0);
    assert!(warnings.is_empty());
}

// 10) tolerant numerics ----------------------------------------------------------

#[test]
fn malformed_numerics_warn_and_preserve_raw_value() {
    let xml = r#"
<skin>
  <elements>
    <bitmap id="bg" file="a.png" x="abc" w="10"/>
    <bitmap id="neg" file="a.png" x="12"/>
  </elements>
  <groupdef id="g">
    <layer id="l" image="bg" x="-5" y="1.5"/>
  </groupdef>
</skin>"#;
    let (skin, warnings) = parse_bytes(&[("skin.xml", xml.as_bytes()), ("a.png", b"\x89PNG")]);

    let bg = &skin.elements.bitmaps[0];
    assert_eq!(bg.x, None, "malformed coordinate must not parse");
    assert_eq!(bg.w, Some(10));
    assert_eq!(
        bg.params.get("x").map(String::as_str),
        Some("abc"),
        "raw value preserved"
    );

    // u32 coords and negative object positions parse
    assert_eq!(skin.elements.bitmaps[1].x, Some(12));
    let layer = match &skin.groups["g"].objects[0] {
        GuiObject::Layer(l) => l,
        other => panic!("expected layer, got {other:?}"),
    };
    assert_eq!(layer.rect.x, Some(-5));
    assert_eq!(layer.rect.y, None);
    assert_eq!(layer.params.get("y").map(String::as_str), Some("1.5"));

    assert_eq!(
        warnings,
        vec![
            Warning::MalformedValue {
                element: "bitmap".into(),
                attribute: "x".into(),
                value: "abc".into(),
            },
            Warning::MalformedValue {
                element: "layer".into(),
                attribute: "y".into(),
                value: "1.5".into(),
            },
        ]
    );
}

// 11) duplicate ids ---------------------------------------------------------------

#[test]
fn duplicate_groupdef_keeps_first() {
    let xml = r#"
<skin>
  <groupdef id="dup" color="red"/>
  <groupdef id="dup" color="blue"/>
</skin>"#;
    let (skin, warnings) = parse_skin_xml(xml);
    assert_eq!(
        skin.groups["dup"].params.get("color").map(String::as_str),
        Some("red")
    );
    assert_eq!(
        warnings,
        vec![Warning::DuplicateId {
            kind: "groupdef".into(),
            id: "dup".into()
        }]
    );
}

// 12) attribute-key casing tolerance on typed fields ------------------------------

#[test]
fn attribute_keys_match_case_insensitively_for_typed_fields() {
    let xml = r#"
<skin>
  <elements>
    <bitmap id="b1" File="a.png"/>
    <bitmap id="b2" file="a.png" X="3"/>
  </elements>
</skin>"#;
    let (skin, warnings) = parse_bytes(&[("skin.xml", xml.as_bytes()), ("a.png", b"\x89PNG")]);
    assert_eq!(skin.elements.bitmaps[0].file.as_deref(), Some("a.png"));
    assert_eq!(skin.elements.bitmaps[1].x, Some(3));
    // original key casing is preserved in nothing (typed fields removed),
    // and no warnings were produced
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
}
