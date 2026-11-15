//! Remote (Navidrome) sync tests: upsert/replace/remove-unseen semantics,
//! user-data preservation, local-row isolation and FTS visibility.

mod common;

use std::collections::HashSet;

use navinamp_library::{Library, RemoteAlbum, RemoteTrack, TrackFilter};

const S: &str = "s1";

fn rt(remote_id: &str, title: &str) -> RemoteTrack {
    RemoteTrack {
        remote_id: remote_id.into(),
        server_id: S.into(),
        remote_album_id: "al-1".into(),
        remote_path: Some("Björk/Debut/01 Human Behaviour.mp3".into()),
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
        label: None,
        catalog_num: None,
    }
}

#[test]
fn sync_stores_per_track_label_and_catalog_number() {
    let (_s, library) = db("remote-labels");
    let mut tagged = rt("t1", "Human Behaviour");
    tagged.label = Some("Nordcore Records".into());
    tagged.catalog_num = Some("NORD 3".into());
    library
        .sync_remote(S, vec![tagged, rt("t2", "Venus as a Boy")], false)
        .unwrap();

    let tracks = library
        .tracks_filtered(&TrackFilter {
            source: Some("navidrome".into()),
            ..TrackFilter::default()
        })
        .unwrap();
    let t1 = tracks
        .iter()
        .find(|t| t.remote_id.as_deref() == Some("t1"))
        .unwrap();
    let t2 = tracks
        .iter()
        .find(|t| t.remote_id.as_deref() == Some("t2"))
        .unwrap();
    assert_eq!(t1.label.as_deref(), Some("Nordcore Records"));
    assert_eq!(t1.catalog_number.as_deref(), Some("NORD 3"));
    assert_eq!(
        t2.label, None,
        "unlabeled rows stay NULL, not empty strings"
    );
    assert_eq!(t2.catalog_number, None);

    // A rescan that drops the tags (file re-tagged without them) rewrites
    // the stored values — the upsert is wholesale, not additive.
    library
        .sync_remote(S, vec![rt("t1", "Human Behaviour")], false)
        .unwrap();
    let tracks = library
        .tracks_filtered(&TrackFilter {
            source: Some("navidrome".into()),
            ..TrackFilter::default()
        })
        .unwrap();
    let t1 = tracks
        .iter()
        .find(|t| t.remote_id.as_deref() == Some("t1"))
        .unwrap();
    assert_eq!(t1.label, None);
    assert_eq!(t1.catalog_number, None);
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
        .sync_remote(
            S,
            vec![rt("t1", "Human Behaviour"), rt("t2", "Venus as a Boy")],
            false,
        )
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
    assert_eq!(first.path, "navidrome://s1/t1");
    assert_eq!(first.source, "navidrome");
    assert_eq!(first.title, "Human Behaviour");
    assert_eq!(
        first.remote_path.as_deref(),
        Some("Björk/Debut/01 Human Behaviour.mp3"),
        "server-relative path is stored alongside the row identity"
    );
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
        .sync_remote(S, vec![rt("t1", "Human Behaviour")], false)
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
    let report = library.sync_remote(S, vec![changed], false).unwrap();

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
        .sync_remote(S, vec![rt("t1", "Human Behaviour")], false)
        .unwrap();
    assert!(!starred_of("t1"), "fresh rows start unstarred");

    let mut starred = rt("t1", "Human Behaviour");
    starred.starred = true;
    library.sync_remote(S, vec![starred], false).unwrap();
    assert!(starred_of("t1"), "a sync refreshes the server-owned star");

    // The star command mirrors the server result into the local row.
    library.set_starred(S, "t1", false).unwrap();
    assert!(!starred_of("t1"));
    library.set_starred(S, "missing-id", true).unwrap(); // silent no-op
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
        .sync_remote(S, vec![rated.clone(), unrated.clone()], false)
        .unwrap();
    assert_eq!(rating_of("t1"), 4);
    assert_eq!(rating_of("t2"), 0);

    // A rating set in the app wins over the server's copy on the next sync.
    library.set_rating("navidrome://s1/t1", 2).unwrap();
    let mut server_changed = rated.clone();
    server_changed.rating = 5;
    let mut server_rated = unrated.clone();
    server_rated.rating = 3;
    library
        .sync_remote(S, vec![server_changed, server_rated], false)
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
        .sync_remote(S, vec![rt("t1", "A"), rt("t2", "B")], false)
        .unwrap();
    // t2 vanishes server-side; t1 stays.
    let report = library.sync_remote(S, vec![rt("t1", "A")], false).unwrap();
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
    let report = library.sync_remote(S, vec![], false).unwrap();
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
    library
        .sync_remote(S, vec![rt("t1", "Böse Spieler")], false)
        .unwrap();

    // Prefix + diacritic-insensitive, exactly like local rows.
    let hits = library.search("bjö spiel", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].remote_id.as_deref(), Some("t1"));

    // Updated titles re-index (trigger-synced FTS).
    let changed = rt("t1", "Big Time Sensuality");
    library.sync_remote(S, vec![changed], false).unwrap();
    assert!(library.search("spiel", 10).unwrap().is_empty());
    let hits = library.search("sensual", 10).unwrap();
    assert_eq!(hits.len(), 1);

    // Removed rows vanish from the index too.
    library.sync_remote(S, vec![], false).unwrap();
    assert!(library.search("sensual", 10).unwrap().is_empty());
}

#[test]
fn empty_remote_ids_are_skipped() {
    let (_s, library) = db("remote-empty-id");
    let report = library
        .sync_remote(S, vec![rt("", "ghost")], false)
        .unwrap();
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
        .sync_remote(S, vec![rt("t1", "A"), rt("t2", "B")], false)
        .unwrap();

    // A mid-walk batch inserts rows but never removes unseen ones.
    let report = library.upsert_remote(&[rt("t3", "C")], false).unwrap();
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
    assert_eq!(library.prune_remote(S, &seen).unwrap(), 1);
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

#[test]
fn remote_ids_are_scoped_per_server() {
    let (_s, library) = db("remote-multi-server");
    let mut other = rt("t1", "Same id on another server");
    other.server_id = "s2".into();

    library
        .sync_remote(S, vec![rt("t1", "On s1")], false)
        .unwrap();
    library.sync_remote("s2", vec![other], false).unwrap();

    let rows = library.tracks_filtered(&TrackFilter::default()).unwrap();
    assert_eq!(rows.len(), 2, "same remote id stays distinct per server");
    let mut paths: Vec<&str> = rows.iter().map(|t| t.path.as_str()).collect();
    paths.sort();
    assert_eq!(paths, vec!["navidrome://s1/t1", "navidrome://s2/t1"]);

    // Per-server counters drive the tree leaves.
    let stats = library.stats().unwrap();
    assert_eq!(stats.navidrome_servers.len(), 2);
    let s1 = stats
        .navidrome_servers
        .iter()
        .find(|s| s.server_id == "s1")
        .unwrap();
    assert_eq!((s1.tracks, s1.albums), (1, 1));

    // Pruning s1 cannot delete s2's row with the same remote id.
    let report = library.sync_remote(S, vec![], false).unwrap();
    assert_eq!(report.removed, 1);
    let s2_rows = library
        .tracks_filtered(&TrackFilter {
            server_id: Some("s2".into()),
            ..TrackFilter::default()
        })
        .unwrap();
    assert_eq!(s2_rows.len(), 1);
    assert_eq!(s2_rows[0].server_id.as_deref(), Some("s2"));

    // Removing a server clears only its rows.
    assert_eq!(library.delete_remote_server("s2").unwrap(), 1);
    assert!(
        library
            .tracks_filtered(&TrackFilter::default())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn legacy_remote_paths_migrate_to_a_server() {
    let (scratch, library) = db("remote-migrate");
    library
        .sync_remote("default", vec![rt("t1", "A")], false)
        .unwrap();
    drop(library);

    // Rewind the row to the pre-aggregation identity.
    {
        let raw = rusqlite::Connection::open(scratch.db()).unwrap();
        raw.execute(
            "UPDATE tracks SET server_id = NULL, path = 'navidrome://t1'
             WHERE remote_id = 't1'",
            [],
        )
        .unwrap();
    }

    let library = Library::open(scratch.db()).unwrap();
    assert_eq!(library.migrate_remote_server("default").unwrap(), 1);
    let track = &library.tracks_filtered(&TrackFilter::default()).unwrap()[0];
    assert_eq!(track.server_id.as_deref(), Some("default"));
    assert_eq!(track.path, "navidrome://default/t1");

    // Idempotent: a second boot rewrites nothing.
    assert_eq!(library.migrate_remote_server("default").unwrap(), 0);
}

#[test]
fn open_migrates_a_tracks_table_created_before_server_id() {
    let (scratch, library) = db("server-id-migration");
    drop(library);
    {
        let raw = rusqlite::Connection::open(scratch.db()).unwrap();
        raw.execute_batch(
            "DROP INDEX idx_tracks_server;
             DROP INDEX idx_tracks_remote_album;
             ALTER TABLE tracks DROP COLUMN server_id;",
        )
        .unwrap();
    }

    // Reopening adds the column + index; remote upserts then work.
    let library = Library::open(scratch.db()).unwrap();
    library.sync_remote(S, vec![rt("t1", "A")], false).unwrap();
    let track = &library.tracks_filtered(&TrackFilter::default()).unwrap()[0];
    assert_eq!(track.server_id.as_deref(), Some("s1"));
}

#[test]
fn remote_album_ids_group_tracks_for_quick_rescans() {
    let (_scratch, library) = db("remote-album-groups");
    let mut other = rt("t3", "C");
    other.remote_album_id = "al-2".into();
    library
        .sync_remote(S, vec![rt("t1", "A"), rt("t2", "B"), other], false)
        .unwrap();

    let by_album = library.remote_track_ids_by_album(S).unwrap();
    assert_eq!(by_album.get("al-1").map(Vec::len), Some(2));
    assert_eq!(by_album.get("al-2"), Some(&vec!["t3".to_string()]));
}

#[test]
fn remote_tracks_filtered_orders_by_server_path_not_synthetic_id() {
    let (_scratch, library) = db("remote-path-order");
    let mut later = rt("t1", "Later album");
    later.remote_path = Some("Z/Album/02 Later.mp3".into());
    later.album = "Z".into();

    let mut earlier = rt("t2", "Earlier album");
    earlier.remote_path = Some("A/Album/01 Earlier.mp3".into());
    earlier.album = "A".into();

    library.sync_remote(S, vec![later, earlier], false).unwrap();
    let paths: Vec<String> = library
        .tracks_filtered(&TrackFilter {
            source: Some("navidrome".into()),
            ..TrackFilter::default()
        })
        .unwrap()
        .into_iter()
        .filter_map(|track| track.remote_path)
        .collect();
    assert_eq!(
        paths,
        vec![
            "A/Album/01 Earlier.mp3".to_string(),
            "Z/Album/02 Later.mp3".to_string(),
        ]
    );
}

#[test]
fn album_fingerprints_round_trip_and_prune() {
    let (_scratch, library) = db("remote-album-fingerprints");
    let first = RemoteAlbum {
        server_id: S.into(),
        album_id: "al-1".into(),
        song_count: 11,
        duration_secs: 2900,
        name: "Debut".into(),
        artist: "Björk".into(),
        year: Some(1993),
        cover_art_id: Some("al-1".into()),
        created: Some("2024-01-01T00:00:00Z".into()),
        label: Some("Bloody Fist Records".into()),
        catalog_num: Some("BF-012".into()),
    };
    let second = RemoteAlbum {
        server_id: S.into(),
        album_id: "al-2".into(),
        song_count: 12,
        duration_secs: 3100,
        name: "Post".into(),
        artist: "Björk".into(),
        year: Some(1995),
        cover_art_id: None,
        created: None,
        label: None,
        catalog_num: None,
    };
    library
        .upsert_remote_albums(&[first.clone(), second])
        .unwrap();

    let albums = library.remote_albums(S).unwrap();
    assert_eq!(albums.len(), 2);
    assert_eq!(albums.get("al-1"), Some(&first));
    assert_eq!(
        albums.get("al-1").unwrap().label.as_deref(),
        Some("Bloody Fist Records"),
        "labels round-trip through the fingerprint table"
    );
    assert_eq!(
        albums.get("al-1").unwrap().catalog_num.as_deref(),
        Some("BF-012")
    );

    let seen: HashSet<String> = ["al-1".to_string()].into_iter().collect();
    assert_eq!(library.prune_remote_albums(S, &seen).unwrap(), 1);
    assert_eq!(library.remote_albums(S).unwrap().len(), 1);

    library.delete_remote_server(S).unwrap();
    assert!(library.remote_albums(S).unwrap().is_empty());
}

#[test]
fn albums_surface_remote_label_and_catalog_number() {
    let (_s, library) = db("remote-album-label-join");
    // Two fingerprint rows share one album_key (server-side duplicate
    // album ids). The view must pick ONE row's pair, never stitch a label
    // from one release with a catalog number from the other.
    let mut other = rt("t2", "Venus as a Boy");
    other.remote_album_id = "al-2".into();
    library
        .sync_remote(S, vec![rt("t1", "Human Behaviour"), other], false)
        .unwrap();
    library
        .upsert_remote_albums(&[
            RemoteAlbum {
                server_id: S.into(),
                album_id: "al-1".into(),
                song_count: 1,
                duration_secs: 250,
                name: "Debut".into(),
                artist: "Björk".into(),
                year: Some(1993),
                cover_art_id: Some("al-1".into()),
                created: None,
                label: Some("Bloody Fist Records".into()),
                catalog_num: Some("BF-012".into()),
            },
            RemoteAlbum {
                server_id: S.into(),
                album_id: "al-2".into(),
                song_count: 1,
                duration_secs: 250,
                name: "Debut".into(),
                artist: "Björk".into(),
                year: Some(1993),
                cover_art_id: Some("al-1".into()),
                created: None,
                label: Some("Alpha Records".into()),
                catalog_num: None,
            },
        ])
        .unwrap();

    let albums = library.albums(Some("navidrome")).unwrap();
    assert_eq!(albums.len(), 1, "one album_key");
    let album = &albums[0];
    assert_eq!(album.track_count, 2);
    // "Bloody Fist Records" sorts after "Alpha Records": the same-row pair
    // wins, so its catalog number comes with it.
    assert_eq!(album.label.as_deref(), Some("Bloody Fist Records"));
    assert_eq!(album.catalog_number.as_deref(), Some("BF-012"));
}

#[test]
fn track_point_lookup_carries_album_label_and_catalog() {
    let (_s, library) = db("remote-track-release-meta");
    library
        .sync_remote(S, vec![rt("t1", "Human Behaviour")], false)
        .unwrap();
    library
        .upsert_remote_albums(&[RemoteAlbum {
            server_id: S.into(),
            album_id: "al-1".into(),
            song_count: 1,
            duration_secs: 250,
            name: "Debut".into(),
            artist: "Björk".into(),
            year: Some(1993),
            cover_art_id: Some("al-1".into()),
            created: None,
            label: Some("Bloody Fist Records".into()),
            catalog_num: Some("BF-012".into()),
        }])
        .unwrap();

    let track = library
        .track_by_path("navidrome://s1/t1")
        .unwrap()
        .expect("remote track");
    assert_eq!(track.label.as_deref(), Some("Bloody Fist Records"));
    assert_eq!(track.catalog_number.as_deref(), Some("BF-012"));

    // Bulk listings stay join-free and omit the release metadata.
    let listed = library
        .tracks_filtered(&TrackFilter {
            source: Some("navidrome".into()),
            ..TrackFilter::default()
        })
        .unwrap();
    assert_eq!(listed[0].label, None);

    // Unknown paths have no row at all.
    assert!(
        library
            .track_by_path("navidrome://s1/nope")
            .unwrap()
            .is_none()
    );
}

#[test]
fn sync_preserves_synced_tags_when_the_native_walk_failed() {
    let (_s, library) = db("remote-tag-preserve");
    let mut tagged = rt("t1", "Human Behaviour");
    tagged.label = Some("Nordcore Records".into());
    tagged.catalog_num = Some("NORD 3".into());
    library.sync_remote(S, vec![tagged], false).unwrap();

    // The next walk could not see the native tag map (server down): the
    // wholesale upsert must keep the previously synced values.
    library
        .sync_remote(S, vec![rt("t1", "Human Behaviour")], true)
        .unwrap();
    let tracks = library
        .tracks_filtered(&TrackFilter {
            source: Some("navidrome".into()),
            ..TrackFilter::default()
        })
        .unwrap();
    let t1 = tracks
        .iter()
        .find(|t| t.remote_id.as_deref() == Some("t1"))
        .unwrap();
    assert_eq!(t1.label.as_deref(), Some("Nordcore Records"));
    assert_eq!(t1.catalog_number.as_deref(), Some("NORD 3"));

    // A run that DID see the tag map (file re-tagged without label) may clear.
    library
        .sync_remote(S, vec![rt("t1", "Human Behaviour")], false)
        .unwrap();
    let tracks = library
        .tracks_filtered(&TrackFilter {
            source: Some("navidrome".into()),
            ..TrackFilter::default()
        })
        .unwrap();
    let t1 = tracks
        .iter()
        .find(|t| t.remote_id.as_deref() == Some("t1"))
        .unwrap();
    assert!(t1.label.is_none() && t1.catalog_number.is_none());
}

#[test]
fn apply_native_tags_backfills_rows_and_falls_back_to_the_album() {
    let (_s, library) = db("remote-backfill");
    // An album fingerprint row carrying the album-level values.
    library
        .upsert_remote_albums(&[RemoteAlbum {
            server_id: S.into(),
            album_id: "al-1".into(),
            song_count: 1,
            duration_secs: 250,
            name: "Debut".into(),
            artist: "Björk".into(),
            year: Some(1993),
            cover_art_id: None,
            created: None,
            label: Some("Apollo Records".into()),
            catalog_num: Some("AMB CD 9205".into()),
        }])
        .unwrap();
    library
        .sync_remote(
            S,
            vec![rt("t1", "Human Behaviour"), rt("t2", "Venus as a Boy")],
            false,
        )
        .unwrap();

    // t1 has its own file tags; t2 has none and takes the album fallback.
    let updated = library
        .apply_native_tags(
            S,
            &[
                (
                    "t1".into(),
                    Some("NORD 3".into()),
                    Some("Nordcore Records".into()),
                ),
                ("t2".into(), None, None),
            ],
        )
        .unwrap();
    assert_eq!(updated, 2);

    let tracks = library
        .tracks_filtered(&TrackFilter {
            source: Some("navidrome".into()),
            ..TrackFilter::default()
        })
        .unwrap();
    let t1 = tracks
        .iter()
        .find(|t| t.remote_id.as_deref() == Some("t1"))
        .unwrap();
    let t2 = tracks
        .iter()
        .find(|t| t.remote_id.as_deref() == Some("t2"))
        .unwrap();
    assert_eq!(t1.label.as_deref(), Some("Nordcore Records"));
    assert_eq!(t1.catalog_number.as_deref(), Some("NORD 3"));
    assert_eq!(
        t2.label.as_deref(),
        Some("Apollo Records"),
        "album fallback"
    );
    assert_eq!(t2.catalog_number.as_deref(), Some("AMB CD 9205"));

    // Unknown ids touch nothing.
    let updated = library
        .apply_native_tags(S, &[("ghost".into(), None, None)])
        .unwrap();
    assert_eq!(updated, 0);
}

#[test]
fn track_by_path_prefers_the_track_tags_over_album_values() {
    let (_s, library) = db("remote-track-by-path-tags");
    let mut own = rt("t1", "Human Behaviour");
    own.label = Some("Nordcore Records".into());
    own.catalog_num = Some("NORD 3".into());
    let fallback = rt("t2", "Venus as a Boy");
    library.upsert_remote(&[own, fallback], false).unwrap();
    library
        .upsert_remote_albums(&[RemoteAlbum {
            server_id: S.into(),
            album_id: "al-1".into(),
            song_count: 2,
            duration_secs: 500,
            name: "Debut".into(),
            artist: "Björk".into(),
            year: Some(1993),
            cover_art_id: None,
            created: None,
            label: Some("Apollo Records".into()),
            catalog_num: Some("AMB CD 9205".into()),
        }])
        .unwrap();

    let paths = |t: &str| {
        library
            .tracks_filtered(&TrackFilter {
                source: Some("navidrome".into()),
                ..TrackFilter::default()
            })
            .unwrap()
            .into_iter()
            .find(|row| row.remote_id.as_deref() == Some(t))
            .unwrap()
            .path
    };

    let own_row = library.track_by_path(&paths("t1")).unwrap().unwrap();
    assert_eq!(
        own_row.label.as_deref(),
        Some("Nordcore Records"),
        "own tag wins"
    );
    assert_eq!(own_row.catalog_number.as_deref(), Some("NORD 3"));

    let fallback_row = library.track_by_path(&paths("t2")).unwrap().unwrap();
    assert_eq!(
        fallback_row.label.as_deref(),
        Some("Apollo Records"),
        "album values fill rows without their own tag"
    );
    assert_eq!(fallback_row.catalog_number.as_deref(), Some("AMB CD 9205"));
}
