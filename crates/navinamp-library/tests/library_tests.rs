//! End-to-end scanner + query tests over temp-dir databases and generated
//! fixtures (see `common`). Fixture validity is pinned separately in
//! `fixture_smoke.rs`.

mod common;

use std::path::PathBuf;
use std::time::SystemTime;

use common::{Meta, Scratch};
use navinamp_library::{Library, RemoteTrack, TrackFilter};

/// A scanned three-file library: two tagged MP3s from one album + one WAV.
fn scanned_library(label: &str) -> (Scratch, Library) {
    let scratch = Scratch::new(label);
    let music = scratch.dir.join("music");
    common::write_mp3(
        &music.join("a1.mp3"),
        100,
        &Meta {
            title: "Böse Spieler".into(),
            artist: "Björk".into(),
            album_artist: "Björk".into(),
            album: "Debut".into(),
            genre: "Electronic".into(),
            year: 1993,
            track: 1,
            disc: 1,
        },
        1_700_000_000,
    );
    common::write_mp3(
        &music.join("a2.mp3"),
        100,
        &Meta {
            title: "Big Time Sensuality".into(),
            artist: "Björk".into(),
            album_artist: "Björk".into(),
            album: "Debut".into(),
            genre: "Electronic".into(),
            year: 1993,
            track: 2,
            disc: 1,
        },
        1_700_000_000,
    );
    common::write_file(
        &music.join("sub/note.wav"),
        &common::wav_bytes(1.5),
        1_700_000_000,
    );

    let library = Library::open(scratch.db()).unwrap();
    let report = library.scan_roots(&[music], |_| {}).unwrap();
    assert_eq!(report.added, 3, "{report:?}");
    assert!(report.errors.is_empty(), "{report:?}");
    (scratch, library)
}

#[test]
fn full_scan_inserts_tags_and_properties() {
    let (_scratch, library) = scanned_library("full-scan");
    let tracks = library
        .tracks_filtered(&TrackFilter {
            album_key: Some(navinamp_library::album_key("Björk", "Björk", "Debut")),
            ..TrackFilter::default()
        })
        .unwrap();
    assert_eq!(tracks.len(), 2);

    let first = &tracks[0];
    assert_eq!(first.title, "Böse Spieler");
    assert_eq!(first.artist, "Björk");
    assert_eq!(first.album_artist, "Björk");
    assert_eq!(first.album, "Debut");
    assert_eq!(first.genre, "Electronic");
    assert_eq!(first.year, Some(1993));
    assert_eq!(first.track_no, Some(1));
    assert_eq!(first.disc_no, Some(1));
    assert_eq!(first.codec, "mp3");
    assert_eq!(first.samplerate, Some(44_100));
    assert_eq!(first.channels, Some(2));
    assert_eq!(first.bitrate_kbps, Some(128));
    assert!((2.5..=2.7).contains(&first.duration_secs));
    assert_eq!(first.source, "local");
    assert_eq!(first.rating, 0);
    assert_eq!(first.play_count, 0);
    assert_eq!(first.art_id, None);
    // album ordering: track 1 before track 2
    assert_eq!(tracks[1].track_no, Some(2));

    // the WAV: no tags, so FTS can't see it — but the row exists with sane
    // properties and an empty-string baseline
    assert!(library.search("note", 10).unwrap().is_empty());
    let wav = library
        .tracks_filtered(&TrackFilter::default())
        .unwrap()
        .into_iter()
        .find(|t| t.codec == "wav")
        .unwrap();
    assert_eq!(wav.title, "");
    assert!((1.4..=1.6).contains(&wav.duration_secs));
    assert_eq!(wav.samplerate, Some(44_100));
}

#[test]
fn rescan_without_changes_is_a_noop() {
    let (scratch, library) = scanned_library("rescan-noop");
    let before = library.stats().unwrap();
    assert_eq!(before.tracks, 3);

    let report = library
        .scan_roots(&[scratch.dir.join("music")], |_| {})
        .unwrap();
    assert_eq!(report.added, 0, "{report:?}");
    assert_eq!(report.updated, 0, "{report:?}");
    assert_eq!(report.removed, 0, "{report:?}");
    assert_eq!(report.unchanged, 3, "{report:?}");
    assert_eq!(report.files_found, 3);
    assert!(report.errors.is_empty(), "{report:?}");
    // `last_scan` advances by design; everything else is untouched.
    let after = library.stats().unwrap();
    assert_eq!(
        (after.tracks, after.albums, after.artists, after.total_secs),
        (
            before.tracks,
            before.albums,
            before.artists,
            before.total_secs
        )
    );
    assert!(after.last_scan >= before.last_scan);
}

#[test]
fn changed_file_is_updated_new_file_added_deleted_file_removed() {
    let (scratch, library) = scanned_library("delta");

    // Rewrite a2.mp3 with a new title AND a later mtime.
    common::write_mp3(
        &scratch.dir.join("music/a2.mp3"),
        100,
        &Meta {
            title: "Venus as a Boy".into(),
            artist: "Björk".into(),
            album_artist: "Björk".into(),
            album: "Debut".into(),
            genre: "Electronic".into(),
            year: 1993,
            track: 2,
            disc: 1,
        },
        1_700_000_500,
    );
    // A brand-new file under the same root.
    common::write_mp3(
        &scratch.dir.join("music/new.mp3"),
        50,
        &Meta {
            title: "New Track".into(),
            artist: "New Artist".into(),
            album: "New Album".into(),
            genre: "Jazz".into(),
            year: 2020,
            track: 1,
            disc: 1,
            ..Meta::default()
        },
        1_700_000_500,
    );
    // Delete one.
    std::fs::remove_file(scratch.dir.join("music/sub/note.wav")).unwrap();

    let report = library
        .scan_roots(&[scratch.dir.join("music")], |_| {})
        .unwrap();
    assert_eq!(report.updated, 1, "{report:?}");
    assert_eq!(report.added, 1, "{report:?}");
    assert_eq!(report.removed, 1, "{report:?}");
    assert_eq!(report.unchanged, 1, "{report:?}"); // a1.mp3

    // The update is visible; the WAV is gone.
    assert_eq!(library.search("Venus", 10).unwrap().len(), 1);
    let all = library.tracks_filtered(&TrackFilter::default()).unwrap();
    let paths: Vec<&str> = all.iter().map(|t| t.path.as_str()).collect();
    assert!(!paths.iter().any(|p| p.ends_with("note.wav")));

    // date_added survives the retag (first-seen, not last-scanned).
    let stats = library.stats().unwrap();
    assert_eq!(stats.tracks, 3);
}

#[test]
fn rows_outside_scanned_roots_survive_removal() {
    let (scratch, library) = scanned_library("outside-roots");
    let music = scratch.dir.join("music");
    let elsewhere = scratch.dir.join("elsewhere");
    common::write_mp3(
        &elsewhere.join("x.mp3"),
        50,
        &Meta {
            title: "Elsewhere".into(),
            artist: "Other".into(),
            album: "Outside".into(),
            genre: "Rock".into(),
            year: 2001,
            track: 1,
            disc: 1,
            ..Meta::default()
        },
        1_700_000_000,
    );
    // Scan both roots so the row exists...
    let report = library
        .scan_roots(&[music.clone(), elsewhere.clone()], |_| {})
        .unwrap();
    assert_eq!(report.added, 1);
    // ...then delete the file and rescan ONLY the music root.
    std::fs::remove_file(elsewhere.join("x.mp3")).unwrap();
    let report = library.scan_roots(&[music], |_| {}).unwrap();
    assert_eq!(report.removed, 0, "{report:?}");
    assert!(
        library
            .tracks_filtered(&TrackFilter::default())
            .unwrap()
            .iter()
            .any(|t| t.title == "Elsewhere")
    );

    // Scanning the other root afterwards does clean it up.
    let report = library.scan_roots(&[elsewhere], |_| {}).unwrap();
    assert_eq!(report.removed, 1, "{report:?}");
}

#[test]
fn fts_search_matches_prefixes_case_and_unicode() {
    let (_scratch, library) = scanned_library("fts");

    // title, case-insensitive
    let hits = library.search("big time", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title, "Big Time Sensuality");

    // artist, prefix only ("bjö" matches "Björk"; unicode61 folds case)
    let hits = library.search("bjö", 10).unwrap();
    assert_eq!(hits.len(), 2);

    // genre
    assert_eq!(library.search("electron", 10).unwrap().len(), 2);
    // multiple terms AND together
    assert_eq!(library.search("björk venus", 10).unwrap().len(), 0);
    assert_eq!(library.search("björk sens", 10).unwrap().len(), 1);

    // umlaut-stripped typing still finds the track (unicode61 folds
    // diacritics for matching)
    assert_eq!(library.search("bos", 10).unwrap().len(), 1);

    // nothing matches / empty / limit
    assert!(library.search("zzz", 10).unwrap().is_empty());
    assert!(library.search("", 10).unwrap().is_empty());
    assert!(library.search("   ", 10).unwrap().is_empty());
    assert_eq!(library.search("björk", 1).unwrap().len(), 1);
}

#[test]
fn albums_group_by_album_key_with_artist_fallback() {
    let scratch = Scratch::new("albums");
    let music = scratch.dir.join("music");
    let meta = |title: &str, artist: &str, album_artist: &str, album: &str, track: u32| Meta {
        title: title.into(),
        artist: artist.into(),
        album_artist: album_artist.into(),
        album: album.into(),
        genre: "Rock".into(),
        year: 1979,
        track,
        disc: 1,
    };
    // Two tracks, no album_artist on one of them -> same key (fallback).
    common::write_mp3(
        &music.join("t1.mp3"),
        50,
        &meta("One", "The Clash", "The Clash", "London Calling", 1),
        1_700_000_000,
    );
    common::write_mp3(
        &music.join("t2.mp3"),
        50,
        &meta("Two", "The Clash", "", "London Calling", 2),
        1_700_000_000,
    );
    // Same album title, different album_artist -> separate album.
    common::write_mp3(
        &music.join("t3.mp3"),
        50,
        &meta("Three", "The Clash", "Various", "London Calling", 3),
        1_700_000_000,
    );

    let library = Library::open(scratch.db()).unwrap();
    library.scan_roots(&[music], |_| {}).unwrap();

    let albums = library.albums().unwrap();
    assert_eq!(albums.len(), 2, "{albums:?}");

    let london = albums
        .iter()
        .find(|a| a.artist == "The Clash")
        .expect("fallback album present");
    assert_eq!(london.name, "London Calling");
    assert_eq!(london.track_count, 2);
    assert_eq!(london.year, Some(1979));
    assert!((london.total_secs - 2.0..=4.0).contains(&london.total_secs)); // 2 × ~2.6 s

    let tracks = library.album_tracks(&london.album_key).unwrap();
    assert_eq!(
        tracks.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
        vec!["One", "Two"]
    );
}

#[test]
fn stats_counts_tracks_albums_artists_and_last_scan() {
    let (scratch, library) = scanned_library("stats");
    let stats = library.stats().unwrap();
    assert_eq!(stats.tracks, 3);
    // Untagged files group under an empty "unknown" album/artist bucket by
    // design (the UI renders it as Unknown Album/Artist), so: Debut + the
    // WAV's bucket, Björk + the empty artist.
    assert_eq!(stats.albums, 2, "{stats:?}");
    assert_eq!(stats.artists, 2);
    assert!(stats.total_secs > 4.0);
    assert!(stats.last_scan.is_some());

    // A fresh database has no last_scan and empty everything.
    let fresh = Library::open(scratch.dir.join("fresh.db")).unwrap();
    assert_eq!(fresh.stats().unwrap().last_scan, None);
}

#[test]
fn corrupt_and_foreign_files_do_not_abort_the_scan() {
    let scratch = Scratch::new("tolerant");
    let music = scratch.dir.join("music");
    common::write_mp3(
        &music.join("good1.mp3"),
        50,
        &Meta {
            title: "Good One".into(),
            artist: "Fine".into(),
            album: "OK".into(),
            genre: "Pop".into(),
            year: 2000,
            track: 1,
            disc: 1,
            ..Meta::default()
        },
        1_700_000_000,
    );
    // Valid extension, garbage content — mid-directory.
    std::fs::create_dir_all(&music).unwrap();
    std::fs::write(music.join("broken.mp3"), b"this is not audio at all").unwrap();
    common::write_mp3(
        &music.join("good2.mp3"),
        50,
        &Meta {
            title: "Good Two".into(),
            artist: "Fine".into(),
            album: "OK".into(),
            genre: "Pop".into(),
            year: 2000,
            track: 2,
            disc: 1,
            ..Meta::default()
        },
        1_700_000_000,
    );
    // Non-audio file: skipped silently, not an error.
    std::fs::write(music.join("cover.png"), b"pretend png").unwrap();

    let library = Library::open(scratch.db()).unwrap();
    let report = library.scan_roots(&[music], |_| {}).unwrap();

    assert_eq!(report.added, 2, "{report:?}");
    assert_eq!(report.files_found, 3); // two good + one broken
    assert_eq!(report.errors.len(), 1);
    assert!(report.errors[0].path.ends_with("broken.mp3"));
    assert_eq!(library.stats().unwrap().tracks, 2);

    // A broken root is reported, not fatal.
    let report = library
        .scan_roots(&[scratch.dir.join("does-not-exist")], |_| {})
        .unwrap();
    assert_eq!(report.errors.len(), 1);
    assert!(report.errors[0].error.contains("cannot resolve root"));
    assert_eq!(report.added + report.updated + report.removed, 0);
}

#[test]
fn tracks_filtered_by_artist_genre_and_paging() {
    let (_scratch, library) = scanned_library("filtered");

    assert_eq!(library.artist_tracks("Björk").unwrap().len(), 2);
    assert!(library.artist_tracks("Nobody").unwrap().is_empty());
    assert_eq!(library.genre_tracks("Electronic").unwrap().len(), 2);

    // combined + limit + offset
    let page = library
        .tracks_filtered(&TrackFilter {
            genre: Some("Electronic".into()),
            limit: Some(1),
            offset: 1,
            ..TrackFilter::default()
        })
        .unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].track_no, Some(2));
}

#[test]
fn progress_reports_discovery_and_per_file_counts() {
    let (scratch, library) = scanned_library("progress-source");
    // now scan a second root to watch the callbacks
    let other = scratch.dir.join("other");
    common::write_mp3(
        &other.join("o.mp3"),
        50,
        &Meta {
            title: "Other".into(),
            artist: "Who".into(),
            album: "What".into(),
            genre: "Pop".into(),
            year: 2010,
            track: 1,
            disc: 1,
            ..Meta::default()
        },
        1_700_000_000,
    );
    let mut events: Vec<navinamp_library::ScanProgress> = Vec::new();
    library.scan_roots(&[other], |p| events.push(p)).unwrap();

    use navinamp_library::ScanProgress;
    assert!(matches!(
        events.first(),
        Some(ScanProgress::Discovered { total: 1 })
    ));
    assert!(matches!(
        events.last(),
        Some(ScanProgress::Progress {
            scanned: 1,
            total: 1
        })
    ));
    assert_eq!(events.len(), 2); // one discovered + one per file
}

#[test]
fn tracks_serialize_camel_case_for_ipc() {
    let (_scratch, library) = scanned_library("ipc-shape");
    let track = &library.search("big time", 1).unwrap()[0];
    let value = serde_json::to_value(track).unwrap();
    let obj = value.as_object().unwrap();
    // spot-check the renamed keys (and that no snake_case leaks out)
    for key in obj.keys() {
        assert!(!key.contains('_'), "key `{key}` is not camelCase");
    }
    assert!(obj.contains_key("durationSecs"));
    assert!(obj.contains_key("albumKey"));
    assert!(obj.contains_key("bitrateKbps"));
    assert!(obj.contains_key("trackNo"));
    assert!(obj.contains_key("lastPlayedAt"));
    assert!(obj.contains_key("dateAdded"));
    assert!(obj.contains_key("remoteId"));
    assert!(obj.contains_key("artId"));
}

#[test]
fn scan_is_visible_to_a_second_connection() {
    // The Tauri layer scans on one connection while queries use another
    // (WAL); a scan through connection A must be visible to B.
    let (scratch, library_a) = scanned_library("two-conns");
    let library_b = Library::open(scratch.db()).unwrap();
    assert_eq!(library_b.stats().unwrap().tracks, 3);

    common::write_mp3(
        &scratch.dir.join("music/later.mp3"),
        50,
        &Meta {
            title: "Later".into(),
            artist: "Björk".into(),
            album_artist: "Björk".into(),
            album: "Debut".into(),
            genre: "Electronic".into(),
            year: 1993,
            track: 3,
            disc: 1,
        },
        1_700_000_900,
    );
    library_a
        .scan_roots(&[scratch.dir.join("music")], |_| {})
        .unwrap();
    assert_eq!(library_b.stats().unwrap().tracks, 4);
}

#[test]
fn nested_and_repeated_roots_do_not_double_count() {
    let (scratch, library) = scanned_library("nested-roots");
    let music = scratch.dir.join("music");
    // music + music/sub + music again: overlapping roots, one row each
    let report = library
        .scan_roots(&[music.clone(), music.join("sub"), music], |_| {})
        .unwrap();
    assert_eq!(report.unchanged, 3, "{report:?}");
    assert_eq!(report.added + report.updated + report.removed, 0);
    assert_eq!(library.stats().unwrap().tracks, 3);
}

#[test]
fn scanning_a_single_file_root_works() {
    let (scratch, library) = scanned_library("file-root");
    let single = PathBuf::from(
        library
            .tracks_filtered(&TrackFilter::default())
            .unwrap()
            .into_iter()
            .find(|t| t.codec == "wav")
            .unwrap()
            .path,
    );
    drop(library);
    // The same DB through a second connection: a file "root" is scanned,
    // and nothing outside it is removed.
    let library = Library::open(scratch.db()).unwrap();
    let report = library.scan_roots(&[single], |_| {}).unwrap();
    assert_eq!(report.unchanged, 1, "{report:?}");
    assert_eq!(library.stats().unwrap().tracks, 3); // nobody else removed
}

// ---------------------------------------------------------------------------
// mark_played (play statistics)
// ---------------------------------------------------------------------------

#[test]
fn mark_played_increments_count_and_sets_timestamp() {
    let (_scratch, library) = scanned_library("mark-played");
    let path = library
        .tracks_filtered(&TrackFilter::default())
        .unwrap()
        .into_iter()
        .find(|t| t.codec == "mp3")
        .unwrap()
        .path;

    let before = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    library.mark_played(&path).unwrap();
    library.mark_played(&path).unwrap();

    let track = library
        .tracks_filtered(&TrackFilter::default())
        .unwrap()
        .into_iter()
        .find(|t| t.path == path)
        .unwrap();
    assert_eq!(track.play_count, 2);
    let last = track.last_played_at.expect("timestamp set");
    assert!(
        last >= before && last <= before + 5,
        "last_played_at {last}"
    );

    // Nothing else moved: the other tracks are untouched.
    let others: Vec<_> = library
        .tracks_filtered(&TrackFilter::default())
        .unwrap()
        .into_iter()
        .filter(|t| t.path != path)
        .collect();
    assert_eq!(others.len(), 2);
    assert!(
        others
            .iter()
            .all(|t| t.play_count == 0 && t.last_played_at.is_none())
    );
}

#[test]
fn mark_played_ignores_unknown_paths_silently() {
    let (_scratch, library) = scanned_library("mark-unknown");
    library.mark_played("/no/such/file.mp3").unwrap();
    library.mark_played("").unwrap();
    let stats = library.stats().unwrap();
    assert_eq!(stats.tracks, 3);
    let any_played = library
        .tracks_filtered(&TrackFilter::default())
        .unwrap()
        .into_iter()
        .any(|t| t.play_count != 0 || t.last_played_at.is_some());
    assert!(!any_played);
}

#[test]
fn set_rating_updates_clamps_and_scopes_by_path() {
    let (_scratch, library) = scanned_library("set-rating");
    let tracks = library.tracks_filtered(&TrackFilter::default()).unwrap();
    let path = tracks[0].path.clone();
    let rating_of = |p: &str| -> i64 {
        library
            .tracks_filtered(&TrackFilter::default())
            .unwrap()
            .into_iter()
            .find(|t| t.path == p)
            .unwrap()
            .rating
    };

    assert_eq!(rating_of(&path), 0);
    library.set_rating(&path, 4).unwrap();
    assert_eq!(rating_of(&path), 4);

    // Out-of-range values clamp to the 0–5 scale.
    library.set_rating(&path, 99).unwrap();
    assert_eq!(rating_of(&path), 5);
    library.set_rating(&path, -3).unwrap();
    assert_eq!(rating_of(&path), 0);

    // Unknown paths are a silent no-op, and other rows are untouched.
    library.set_rating("/no/such/file.mp3", 5).unwrap();
    let others: Vec<_> = library
        .tracks_filtered(&TrackFilter::default())
        .unwrap()
        .into_iter()
        .filter(|t| t.path != path)
        .collect();
    assert!(others.iter().all(|t| t.rating == 0));
}

#[test]
fn mark_played_never_touches_remote_rows() {
    let (_scratch, library) = scanned_library("mark-remote");
    library
        .sync_remote(vec![RemoteTrack {
            remote_id: "t9".into(),
            title: "Remote Song".into(),
            artist: "Björk".into(),
            album_artist: "Björk".into(),
            album: "Debut".into(),
            genre: "Electronic".into(),
            year: Some(1993),
            track_no: Some(9),
            disc_no: Some(1),
            duration_secs: 250.0,
            bitrate_kbps: Some(320),
            samplerate: Some(44_100),
            channels: Some(2),
            codec: "mp3".into(),
            rating: 0,
            art_id: None,
            size: Some(1_000_000),
        }])
        .unwrap();
    let remote_path = library
        .tracks_filtered(&TrackFilter {
            source: Some("navidrome".into()),
            ..TrackFilter::default()
        })
        .unwrap()[0]
        .path
        .clone();

    // Same path, but the row is remote: the source guard must skip it.
    library.mark_played(&remote_path).unwrap();
    let remote = library
        .tracks_filtered(&TrackFilter {
            source: Some("navidrome".into()),
            ..TrackFilter::default()
        })
        .unwrap()
        .into_iter()
        .find(|t| t.path == remote_path)
        .unwrap();
    assert_eq!(remote.play_count, 0);
    assert_eq!(remote.last_played_at, None);
}
