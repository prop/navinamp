//! Fixture sanity: prove lofty parses our hand-crafted files BEFORE the
//! scanner tests lean on them. If this fails, the fixture generators are
//! wrong — not lofty, not the scanner.

mod common;

use common::{Meta, Scratch};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::Accessor;

#[test]
fn lofty_reads_handmade_mp3_frames_and_id3_tag() {
    let scratch = Scratch::new("mp3-smoke");
    let path = scratch.path("smoke.mp3");
    common::write_mp3(
        &path,
        100, // 100 * 1152 / 44100 = 2.607... s
        &Meta {
            title: "Böse Spieler".into(),
            artist: "Björk".into(),
            album_artist: "Björk".into(),
            album: "Debut".into(),
            genre: "Electronic".into(),
            year: 1993,
            track: 3,
            disc: 1,
        },
        1_700_000_000,
    );

    let tagged = lofty::read_from_path(&path).expect("lofty must read our MP3");
    let props = tagged.properties();
    let tag = tagged
        .primary_tag()
        .or_else(|| tagged.first_tag())
        .expect("ID3v2 tag must round-trip");

    assert_eq!(tagged.file_type(), lofty::file::FileType::Mpeg);
    assert_eq!(tag.title().as_deref(), Some("Böse Spieler"));
    assert_eq!(tag.artist().as_deref(), Some("Björk"));
    assert_eq!(tag.album().as_deref(), Some("Debut"));
    assert_eq!(tag.genre().as_deref(), Some("Electronic"));
    assert_eq!(tag.date().map(|d| d.year), Some(1993));
    assert_eq!(tag.track(), Some(3));
    assert_eq!(tag.disk(), Some(1));

    let duration = props.duration().as_secs_f64();
    assert!(
        (2.5..=2.7).contains(&duration),
        "duration {duration}s should be ~2.61 s (100 × 1152/44100)"
    );
    assert_eq!(props.sample_rate(), Some(44_100));
    assert_eq!(props.channels(), Some(2));
    assert!(props.audio_bitrate().unwrap_or(0) > 0);
}

#[test]
fn lofty_reads_handmade_wav() {
    let scratch = Scratch::new("wav-smoke");
    let path = scratch.path("smoke.wav");
    common::write_file(&path, &common::wav_bytes(1.5), 1_700_000_000);

    let tagged = match lofty::read_from_path(&path) {
        Ok(t) => t,
        Err(e) => panic!("lofty must read our WAV: {e}"),
    };
    let props = tagged.properties();
    assert_eq!(tagged.file_type(), lofty::file::FileType::Wav);
    let duration = props.duration().as_secs_f64();
    assert!(
        (1.4..=1.6).contains(&duration),
        "duration {duration}s should be ~1.5 s by construction"
    );
    assert_eq!(props.sample_rate(), Some(44_100));
    assert_eq!(props.channels(), Some(2));
}
