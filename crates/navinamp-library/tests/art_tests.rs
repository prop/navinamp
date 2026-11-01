//! Album-art pipeline tests: embedded extraction, folder-image fallback,
//! content-addressed dedup, album art ids, incremental behavior and the
//! one-time backfill.

mod common;

use std::path::Path;

use lofty::picture::MimeType;
use navinamp_library::{Library, RemoteTrack, TrackFilter};
use sha2::{Digest, Sha256};

/// Expected art id for `bytes`: sha256[..8] hex — pins the cache naming.
fn content_id(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

fn meta(title: &str, album: &str, track: u32) -> common::Meta {
    common::Meta {
        title: title.into(),
        artist: "Björk".into(),
        album_artist: "Björk".into(),
        album: album.into(),
        genre: "Electronic".into(),
        year: 1993,
        track,
        disc: 1,
    }
}

/// A track with `title`/`album` tags and (optionally) an embedded picture,
/// stamped with a distinct mtime so rescans see exactly what the test wants.
fn write_track(
    dir: &Path,
    file: &str,
    title: &str,
    album: &str,
    track: u32,
    mtime: i64,
    art: Option<(&[u8], MimeType)>,
) {
    let path = dir.join(file);
    common::write_mp3(&path, 20, &meta(title, album, track), mtime);
    if let Some((bytes, mime)) = art {
        common::attach_picture(&path, mime, bytes);
    }
}

fn remote(remote_id: &str, album: &str, art_id: Option<&str>) -> RemoteTrack {
    RemoteTrack {
        remote_id: remote_id.into(),
        title: remote_id.into(),
        artist: "Björk".into(),
        album_artist: "Björk".into(),
        album: album.into(),
        genre: "Electronic".into(),
        year: Some(1993),
        track_no: Some(1),
        disc_no: Some(1),
        duration_secs: 200.0,
        bitrate_kbps: Some(320),
        samplerate: Some(44_100),
        channels: Some(2),
        codec: "mp3".into(),
        rating: 0,
        starred: false,
        art_id: art_id.map(str::to_owned),
        size: Some(1_000),
    }
}

#[test]
fn embedded_art_is_extracted_cached_and_served_by_id() {
    let scratch = common::Scratch::new("art-embedded");
    let music = scratch.dir.join("music");
    std::fs::create_dir_all(&music).unwrap();
    let png = common::png_bytes();
    write_track(
        &music,
        "a.mp3",
        "Human Behaviour",
        "Debut",
        1,
        1_700_000_000,
        Some((&png, MimeType::Png)),
    );

    let library = Library::open(scratch.db()).unwrap();
    let report = library.scan_roots(&[music], |_| {}).unwrap();
    assert_eq!(report.added, 1);
    assert!(report.errors.is_empty());

    let id = content_id(&png);
    let tracks = library.tracks_filtered(&TrackFilter::default()).unwrap();
    assert_eq!(tracks[0].art_id.as_deref(), Some(id.as_str()));

    // The cache lives next to the db, one file per unique content, bytes
    // intact — exactly what the serving side reads back.
    let art_dir = scratch.dir.join("art-cache");
    assert_eq!(
        std::fs::read(art_dir.join(format!("{id}.png"))).unwrap(),
        png,
        "embedded PNG must land in the content-addressed cache"
    );
    assert_eq!(std::fs::read_dir(&art_dir).unwrap().count(), 1);
}

#[test]
fn folder_art_fills_files_without_embedded_art() {
    let scratch = common::Scratch::new("art-folder");
    let music = scratch.dir.join("music");
    std::fs::create_dir_all(&music).unwrap();

    let png = common::png_bytes();
    let jpg = common::jpeg_bytes();
    write_track(
        &music,
        "embedded.mp3",
        "A",
        "Debut",
        1,
        1_700_000_000,
        Some((&png, MimeType::Png)),
    );
    write_track(&music, "bare.mp3", "B", "Debut", 2, 1_700_000_000, None);
    // Case-insensitive lookup: `Cover.JPG` must be found as cover.jpg.
    common::write_file(&music.join("Cover.JPG"), &jpg, 1_700_000_000);

    let library = Library::open(scratch.db()).unwrap();
    library.scan_roots(&[music], |_| {}).unwrap();

    let tracks = library.tracks_filtered(&TrackFilter::default()).unwrap();
    let embedded = tracks.iter().find(|t| t.title == "A").unwrap();
    let bare = tracks.iter().find(|t| t.title == "B").unwrap();

    // Embedded wins for the file that has it; the folder image fills the
    // file that doesn't — and both cache under their own content ids.
    assert_eq!(embedded.art_id.as_deref(), Some(content_id(&png).as_str()));
    assert_eq!(bare.art_id.as_deref(), Some(content_id(&jpg).as_str()));
    let art_dir = scratch.dir.join("art-cache");
    assert_eq!(std::fs::read_dir(&art_dir).unwrap().count(), 2);
}

#[test]
fn identical_art_content_is_cached_once() {
    let scratch = common::Scratch::new("art-dedup");
    let a = scratch.dir.join("a");
    let b = scratch.dir.join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();

    let jpg = common::jpeg_bytes();
    // Same folder image in two directories, plus two files sharing one
    // embedded picture — four art-bearing rows, one cache file.
    common::write_file(&a.join("folder.jpg"), &jpg, 1_700_000_000);
    common::write_file(&b.join("cover.jpg"), &jpg, 1_700_000_000);
    let png = common::png_bytes();
    write_track(
        &a,
        "one.mp3",
        "One",
        "Debut",
        1,
        1_700_000_000,
        Some((&png, MimeType::Png)),
    );
    write_track(
        &a,
        "two.mp3",
        "Two",
        "Debut",
        2,
        1_700_000_000,
        Some((&png, MimeType::Png)),
    );
    write_track(&b, "three.mp3", "Three", "Post", 1, 1_700_000_000, None);
    write_track(&b, "four.mp3", "Four", "Post", 2, 1_700_000_000, None);

    let library = Library::open(scratch.db()).unwrap();
    library
        .scan_roots(std::slice::from_ref(&scratch.dir), |_| {})
        .unwrap();

    let tracks = library.tracks_filtered(&TrackFilter::default()).unwrap();
    let mut titles: Vec<String> = tracks.iter().map(|t| t.title.clone()).collect();
    titles.sort();
    assert_eq!(titles, vec!["Four", "One", "Three", "Two"]);

    let folder_id = content_id(&jpg);
    let embedded_id = content_id(&png);
    for t in &tracks {
        let expected = if t.title == "Three" || t.title == "Four" {
            &folder_id
        } else {
            &embedded_id
        };
        assert_eq!(t.art_id.as_deref(), Some(expected.as_str()), "{}", t.title);
    }

    let art_dir = scratch.dir.join("art-cache");
    assert_eq!(
        std::fs::read_dir(&art_dir).unwrap().count(),
        2,
        "four art-bearing rows, two unique contents"
    );
}

#[test]
fn albums_expose_first_track_art_and_its_source() {
    let scratch = common::Scratch::new("art-albums");
    let music = scratch.dir.join("music");
    std::fs::create_dir_all(&music).unwrap();

    // Album "Debut": track 1 bare, track 2 with embedded art — the album's
    // art comes from the first track that HAS art (disc/track order).
    let png = common::png_bytes();
    write_track(&music, "d1.mp3", "D1", "Debut", 1, 1_700_000_000, None);
    write_track(
        &music,
        "d2.mp3",
        "D2",
        "Debut",
        2,
        1_700_000_000,
        Some((&png, MimeType::Png)),
    );
    // Album "Post": no art anywhere.
    write_track(&music, "p1.mp3", "P1", "Post", 1, 1_700_000_000, None);

    let library = Library::open(scratch.db()).unwrap();
    library.scan_roots(&[music], |_| {}).unwrap();

    // A navidrome album whose rows carry the server's coverArt id.
    library
        .sync_remote(vec![remote("mf-1", "Homogenic", Some("al-42"))])
        .unwrap();

    let albums = library.albums(None).unwrap();
    let find = |name: &str| {
        albums
            .iter()
            .find(|a| a.name == name)
            .unwrap_or_else(|| panic!("album {name} missing"))
    };

    let debut = find("Debut");
    assert_eq!(debut.art_id.as_deref(), Some(content_id(&png).as_str()));
    assert_eq!(debut.art_source.as_deref(), Some("local"));

    let post = find("Post");
    assert_eq!(post.art_id, None);
    assert_eq!(post.art_source, None);

    let homogenic = find("Homogenic");
    assert_eq!(homogenic.art_id.as_deref(), Some("al-42"));
    assert_eq!(homogenic.art_source.as_deref(), Some("navidrome"));
}

#[test]
fn unchanged_files_keep_art_and_rescans_do_not_duplicate() {
    let scratch = common::Scratch::new("art-incremental");
    let music = scratch.dir.join("music");
    std::fs::create_dir_all(&music).unwrap();
    let png = common::png_bytes();
    let id = content_id(&png);
    write_track(
        &music,
        "a.mp3",
        "A",
        "Debut",
        1,
        1_700_000_000,
        Some((&png, MimeType::Png)),
    );

    let library = Library::open(scratch.db()).unwrap();
    library
        .scan_roots(std::slice::from_ref(&music), |_| {})
        .unwrap();
    let art_dir = scratch.dir.join("art-cache");
    assert_eq!(std::fs::read_dir(&art_dir).unwrap().count(), 1);

    // Unchanged rescan: art kept, cache untouched.
    let report = library
        .scan_roots(std::slice::from_ref(&music), |_| {})
        .unwrap();
    assert_eq!(report.unchanged, 1);
    let tracks = library.tracks_filtered(&TrackFilter::default()).unwrap();
    assert_eq!(tracks[0].art_id.as_deref(), Some(id.as_str()));
    assert_eq!(std::fs::read_dir(&art_dir).unwrap().count(), 1);

    // Re-tag with different art (new mtime → changed): art_id follows.
    let jpg = common::jpeg_bytes();
    write_track(
        &music,
        "a.mp3",
        "A",
        "Debut",
        1,
        1_800_000_000,
        Some((&jpg, MimeType::Jpeg)),
    );
    let report = library
        .scan_roots(std::slice::from_ref(&music), |_| {})
        .unwrap();
    assert_eq!(report.updated, 1);
    let tracks = library.tracks_filtered(&TrackFilter::default()).unwrap();
    assert_eq!(
        tracks[0].art_id.as_deref(),
        Some(content_id(&jpg).as_str()),
        "changed files re-derive their art"
    );
    assert_eq!(std::fs::read_dir(&art_dir).unwrap().count(), 2);

    // Art taken away entirely: changed file falls back to NULL (no folder
    // image in this directory).
    write_track(&music, "a.mp3", "A", "Debut", 1, 1_900_000_000, None);
    library
        .scan_roots(std::slice::from_ref(&music), |_| {})
        .unwrap();
    let tracks = library.tracks_filtered(&TrackFilter::default()).unwrap();
    assert_eq!(tracks[0].art_id, None);
}

#[test]
fn first_art_aware_scan_backfills_pre_art_rows() {
    let scratch = common::Scratch::new("art-backfill");
    let music = scratch.dir.join("music");
    std::fs::create_dir_all(&music).unwrap();
    let png = common::png_bytes();
    let id = content_id(&png);
    write_track(
        &music,
        "a.mp3",
        "A",
        "Debut",
        1,
        1_700_000_000,
        Some((&png, MimeType::Png)),
    );

    let library = Library::open(scratch.db()).unwrap();
    library
        .scan_roots(std::slice::from_ref(&music), |_| {})
        .unwrap();

    // Simulate a database written by a pre-art build: art nulled out, the
    // backfill marker absent. Files are unchanged on disk.
    let raw = rusqlite::Connection::open(scratch.db()).unwrap();
    raw.execute("UPDATE tracks SET art_id = NULL", []).unwrap();
    raw.execute("DELETE FROM meta WHERE key = 'art_backfilled'", [])
        .unwrap();
    drop(raw);

    let report = library
        .scan_roots(std::slice::from_ref(&music), |_| {})
        .unwrap();
    assert_eq!(report.unchanged, 1, "the file itself never changed");
    let tracks = library.tracks_filtered(&TrackFilter::default()).unwrap();
    assert_eq!(
        tracks[0].art_id.as_deref(),
        Some(id.as_str()),
        "the one-time backfill restores art for unchanged rows"
    );

    // And it was one-time: clearing art again WITHOUT clearing the marker
    // leaves rows alone (that's the documented steady-state behavior).
    let raw = rusqlite::Connection::open(scratch.db()).unwrap();
    raw.execute("UPDATE tracks SET art_id = NULL", []).unwrap();
    drop(raw);
    library.scan_roots(&[music], |_| {}).unwrap();
    let tracks = library.tracks_filtered(&TrackFilter::default()).unwrap();
    assert_eq!(tracks[0].art_id, None);
}

#[test]
fn art_for_track_point_lookup_covers_both_sources() {
    let scratch = common::Scratch::new("art-lookup");
    let music = scratch.dir.join("music");
    std::fs::create_dir_all(&music).unwrap();
    let png = common::png_bytes();
    write_track(
        &music,
        "a.mp3",
        "A",
        "Debut",
        1,
        1_700_000_000,
        Some((&png, MimeType::Png)),
    );

    let library = Library::open(scratch.db()).unwrap();
    library
        .scan_roots(std::slice::from_ref(&music), |_| {})
        .unwrap();
    library
        .sync_remote(vec![remote("mf-7", "Post", None)])
        .unwrap();

    // The scanner canonicalizes roots, so the stored path is the resolved
    // one — canonicalize on this side too before the point lookup.
    let local_path = music
        .join("a.mp3")
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        library.art_for_track(&local_path).unwrap().as_deref(),
        Some(content_id(&png).as_str())
    );
    assert_eq!(
        library.art_for_track("navidrome://mf-7").unwrap(),
        None,
        "this remote row has no art"
    );
    assert_eq!(library.art_for_track("/no/such/track.mp3").unwrap(), None);
}

#[test]
fn in_memory_libraries_scan_without_art() {
    let scratch = common::Scratch::new("art-memory");
    let music = scratch.dir.join("music");
    std::fs::create_dir_all(&music).unwrap();
    let png = common::png_bytes();
    write_track(
        &music,
        "a.mp3",
        "A",
        "Debut",
        1,
        1_700_000_000,
        Some((&png, MimeType::Png)),
    );

    let library = Library::open(":memory:").unwrap();
    assert_eq!(library.art_dir(), None);
    library.scan_roots(&[music], |_| {}).unwrap();

    // Tracks still index; art is skipped rather than cached.
    let tracks = library.tracks_filtered(&TrackFilter::default()).unwrap();
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].art_id, None);
    assert!(!scratch.dir.join("art-cache").exists());
}
