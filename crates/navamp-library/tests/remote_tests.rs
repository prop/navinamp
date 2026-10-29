//! Remote (Navidrome) sync tests: upsert/replace/remove-unseen semantics,
//! user-data preservation, local-row isolation and FTS visibility.

mod common;

use navamp_library::{Library, RemoteTrack, TrackFilter};

fn rt(remote_id: &str, title: &str) -> RemoteTrack {
    RemoteTrack {
        remote_id: remote_id.into(),
        title: title.into(),
        artist: "Björk".into(),
        album_artist: "Björk".into(),
        album: "Debut".into(),
        genre: "Electronic".into(),
        year: Some(1993),
        track_no: Some(1),
        disc_no: Some(1),
        duration_secs: 250.0,
        bitrate_kbps: Some(320),
        samplerate: Some(44_100),
        channels: Some(2),
        codec: "mp3".into(),
        art_id: Some("al-1".into()),
        size: Some(10_000_000),
    }
}

fn db(label: &str) -> (common::Scratch, Library) {
    let scratch = common::Scratch::new(label);
    let library = Library::open(scratch.db()).unwrap();
    (scratch, library)
}

#[test]
fn sync_inserts_remote_rows_with_derived_paths() {
    let (_s, library) = db("remote-insert");
    let report = library
        .sync_remote(vec![
            rt("t1", "Human Behaviour"),
            rt("t2", "Venus as a Boy"),
        ])
        .unwrap();

    assert_eq!(report.added, 2);
    assert_eq!(report.updated, 0);
    assert_eq!(report.removed, 0);

    let tracks = library
        .tracks_filtered(&TrackFilter {
            source: Some("navidrome".into()),
            ..TrackFilter::default()
        })
        .unwrap();
    assert_eq!(tracks.len(), 2);

    let first = tracks
        .iter()
        .find(|t| t.remote_id.as_deref() == Some("t1"))
        .unwrap();
    assert_eq!(first.path, "navidrome://t1");
    assert_eq!(first.source, "navidrome");
    assert_eq!(first.title, "Human Behaviour");
    assert_eq!(first.art_id.as_deref(), Some("al-1"));
    assert_eq!(first.codec, "mp3");
    assert_eq!(first.bitrate_kbps, Some(320));
    assert_eq!(first.size, 10_000_000);
    assert_eq!(first.mtime, 0, "remote rows carry no filesystem stamp");
    assert_eq!(
        first.album_key,
        navamp_library::album_key("Björk", "Björk", "Debut")
    );

    // The source filter also excludes: no locals exist, so unfiltered == 2.
    assert_eq!(
        library
            .tracks_filtered(&TrackFilter::default())
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn resync_updates_metadata_preserves_user_data_and_dates() {
    let (scratch, library) = db("remote-update");
    library
        .sync_remote(vec![rt("t1", "Human Behaviour")])
        .unwrap();

    // Simulate user data + an older date_added (direct SQL: there is no
    // write API for user data yet — same approach as smart_tests.rs).
    let raw = rusqlite::Connection::open(scratch.db()).unwrap();
    raw.execute(
        "UPDATE tracks SET rating = 5, play_count = 7, last_played_at = 1750000000,
                           date_added = 1000, source = 'navidrome'
         WHERE remote_id = 't1'",
        [],
    )
    .unwrap();
    drop(raw);

    let mut changed = rt("t1", "Human Behaviour (Remaster)");
    changed.album = "Debut (Deluxe)".into();
    changed.art_id = Some("al-2".into());
    changed.duration_secs = 251.0;
    let report = library.sync_remote(vec![changed]).unwrap();

    assert_eq!(report.added, 0);
    assert_eq!(report.updated, 1);
    assert_eq!(report.removed, 0);

    let t = &library.tracks_filtered(&TrackFilter::default()).unwrap()[0];
    assert_eq!(t.title, "Human Behaviour (Remaster)");
    assert_eq!(t.album, "Debut (Deluxe)");
    assert_eq!(t.art_id.as_deref(), Some("al-2"), "art_id must refresh");
    assert_eq!(t.duration_secs, 251.0);
    // Survivors:
    assert_eq!(t.rating, 5);
    assert_eq!(t.play_count, 7);
    assert_eq!(t.last_played_at, Some(1_750_000_000));
    assert_eq!(t.date_added, 1000, "date_added survives a resync");
}

#[test]
fn remove_unseen_is_scoped_to_navidrome_rows() {
    let (scratch, library) = db("remote-remove");
    // A local track under the same DB.
    let music = scratch.dir.join("music");
    common::write_mp3(
        &music.join("local.mp3"),
        20,
        &common::Meta {
            title: "Local Song".into(),
            artist: "Björk".into(),
            ..common::Meta::default()
        },
        1_700_000_000,
    );
    library.scan_roots(&[music], |_| {}).unwrap();

    library
        .sync_remote(vec![rt("t1", "A"), rt("t2", "B")])
        .unwrap();
    // t2 vanishes server-side; t1 stays.
    let report = library.sync_remote(vec![rt("t1", "A")]).unwrap();
    assert_eq!(report.removed, 1);
    assert_eq!(report.added, 0);
    assert_eq!(report.updated, 1);

    let remote = library
        .tracks_filtered(&TrackFilter {
            source: Some("navidrome".into()),
            ..TrackFilter::default()
        })
        .unwrap();
    let remote_ids: Vec<&str> = remote
        .iter()
        .map(|t| t.remote_id.as_deref().unwrap())
        .collect();
    assert_eq!(remote_ids, vec!["t1"]);

    // The local row is untouched — same row content as the scan left it.
    let locals = library
        .tracks_filtered(&TrackFilter {
            source: Some("local".into()),
            ..TrackFilter::default()
        })
        .unwrap();
    assert_eq!(locals.len(), 1);
    assert_eq!(locals[0].title, "Local Song");
    assert_eq!(locals[0].play_count, 0);

    // And an empty sync wipes the remote side only.
    let report = library.sync_remote(vec![]).unwrap();
    assert_eq!(report.removed, 1);
    assert_eq!(
        library
            .tracks_filtered(&TrackFilter {
                source: Some("navidrome".into()),
                ..TrackFilter::default()
            })
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        library
            .tracks_filtered(&TrackFilter::default())
            .unwrap()
            .len(),
        1,
        "the local row must survive an empty remote sync"
    );
}

#[test]
fn fts_finds_remote_rows() {
    let (_s, library) = db("remote-fts");
    library.sync_remote(vec![rt("t1", "Böse Spieler")]).unwrap();

    // Prefix + diacritic-insensitive, exactly like local rows.
    let hits = library.search("bjö spiel", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].remote_id.as_deref(), Some("t1"));

    // Updated titles re-index (trigger-synced FTS).
    let changed = rt("t1", "Big Time Sensuality");
    library.sync_remote(vec![changed]).unwrap();
    assert!(library.search("spiel", 10).unwrap().is_empty());
    let hits = library.search("sensual", 10).unwrap();
    assert_eq!(hits.len(), 1);

    // Removed rows vanish from the index too.
    library.sync_remote(vec![]).unwrap();
    assert!(library.search("sensual", 10).unwrap().is_empty());
}

#[test]
fn empty_remote_ids_are_skipped() {
    let (_s, library) = db("remote-empty-id");
    let report = library.sync_remote(vec![rt("", "ghost")]).unwrap();
    assert_eq!(report.added, 0);
    assert_eq!(
        library
            .tracks_filtered(&TrackFilter::default())
            .unwrap()
            .len(),
        0
    );
}
