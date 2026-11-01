//! Remote (Navidrome) sync tests: upsert/replace/remove-unseen semantics,
//! user-data preservation, local-row isolation and FTS visibility.

mod common;

use navinamp_library::{Library, RemoteTrack, TrackFilter};

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
        rating: 0,
        starred: false,
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
        navinamp_library::album_key("Björk", "Björk", "Debut")
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
fn sync_refreshes_server_starred_state() {
    let (_scratch, library) = db("remote-starred");
    let starred_of = |remote_id: &str| -> bool {
        library
            .tracks_filtered(&TrackFilter::default())
            .unwrap()
            .into_iter()
            .find(|t| t.remote_id.as_deref() == Some(remote_id))
            .unwrap()
            .starred
    };

    library
        .sync_remote(vec![rt("t1", "Human Behaviour")])
        .unwrap();
    assert!(!starred_of("t1"), "fresh rows start unstarred");

    let mut starred = rt("t1", "Human Behaviour");
    starred.starred = true;
    library.sync_remote(vec![starred]).unwrap();
    assert!(starred_of("t1"), "a sync refreshes the server-owned star");

    // The star command mirrors the server result into the local row.
    library.set_starred("t1", false).unwrap();
    assert!(!starred_of("t1"));
    library.set_starred("missing-id", true).unwrap(); // silent no-op
}

#[test]
fn sync_imports_server_ratings_without_clobbering_local_ones() {
    let (_scratch, library) = db("remote-ratings");
    let rating_of = |remote_id: &str| -> i64 {
        library
            .tracks_filtered(&TrackFilter::default())
            .unwrap()
            .into_iter()
            .find(|t| t.remote_id.as_deref() == Some(remote_id))
            .unwrap()
            .rating
    };

    // Server ratings seed fresh rows; unrated songs stay 0.
    let mut rated = rt("t1", "Human Behaviour");
    rated.rating = 4;
    let unrated = rt("t2", "Venus as a Boy");
    library
        .sync_remote(vec![rated.clone(), unrated.clone()])
        .unwrap();
    assert_eq!(rating_of("t1"), 4);
    assert_eq!(rating_of("t2"), 0);

    // A rating set in the app wins over the server's copy on the next sync.
    library.set_rating("navidrome://t1", 2).unwrap();
    let mut server_changed = rated.clone();
    server_changed.rating = 5;
    let mut server_rated = unrated.clone();
    server_rated.rating = 3;
    library
        .sync_remote(vec![server_changed, server_rated])
        .unwrap();
    assert_eq!(rating_of("t1"), 2, "local rating survives a resync");
    assert_eq!(rating_of("t2"), 3, "unrated row picks up the server rating");
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

#[test]
fn batched_upserts_defer_pruning_until_the_walk_completes() {
    let (_scratch, library) = db("remote-batched");
    library
        .sync_remote(vec![rt("t1", "A"), rt("t2", "B")])
        .unwrap();

    // A mid-walk batch inserts rows but never removes unseen ones.
    let report = library.upsert_remote(&[rt("t3", "C")]).unwrap();
    assert_eq!((report.added, report.updated, report.removed), (1, 0, 0));
    assert_eq!(
        library
            .tracks_filtered(&TrackFilter {
                source: Some("navidrome".into()),
                ..TrackFilter::default()
            })
            .unwrap()
            .len(),
        3
    );

    // The final prune (complete walk) removes rows missing from `seen`.
    let seen: std::collections::HashSet<String> =
        ["t1", "t3"].iter().map(|id| (*id).to_string()).collect();
    assert_eq!(library.prune_remote(&seen).unwrap(), 1);
    let ids: Vec<String> = library
        .tracks_filtered(&TrackFilter {
            source: Some("navidrome".into()),
            ..TrackFilter::default()
        })
        .unwrap()
        .into_iter()
        .filter_map(|t| t.remote_id)
        .collect();
    assert_eq!(ids, vec!["t1", "t3"]);
}
