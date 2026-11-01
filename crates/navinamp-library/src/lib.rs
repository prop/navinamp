//! NaVinaMP music library: an incremental file scanner over a single SQLite
//! database with an FTS5 search index.
//!
//! The crate is deliberately free of Tauri/app concerns: give it a database
//! path (or `:memory:`), scan some directories, query what you got. The
//! Tauri shell in `src-tauri/src/library.rs` is a thin wrapper.
//!
//! ```no_run
//! use std::path::PathBuf;
//! use navinamp_library::Library;
//!
//! let library = Library::open("/somewhere/navinamp.db").unwrap();
//! let report = library
//!     .scan_roots(&[PathBuf::from("~/Music")], |_| {})
//!     .unwrap();
//! println!("{} added, {} updated", report.added, report.updated);
//!
//! for track in library.search("beatl", 25).unwrap() {
//!     println!("{} — {}", track.artist, track.title);
//! }
//! ```
//!
//! Concurrency: [`Library`] is `Send` but not `Sync` (rusqlite's `Connection`
//! isn't `Sync`). The bridge guards it with a `Mutex`; the scanner's
//! transaction means one [`Library::scan_roots`] at a time per connection.
//! Opening a second `Library` on the same file is supported — `open` enables
//! WAL and a busy timeout, which is how the app keeps queries responsive
//! while a background scan connection writes.

/// `has_audio_extension` companion: expand user-picked paths into playable
/// audio files. Directories are walked recursively (deterministic order),
/// keeping only known-audio files; plain files pass through the same
/// extension filter. The queue's "add folder" flows through this.
pub fn expand_paths(paths: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for raw in paths {
        let path = std::path::Path::new(raw);
        if path.is_dir() {
            for entry in walkdir::WalkDir::new(path)
                .sort_by_file_name()
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if entry.file_type().is_file()
                    && scanner::has_audio_extension(entry.path())
                    && let Some(p) = entry.path().to_str()
                {
                    out.push(p.to_owned());
                }
            }
        } else if scanner::has_audio_extension(path) {
            out.push(raw.clone());
        }
    }
    out
}

mod art;
mod model;
mod query;
mod remote;
mod scanner;
mod schema;
mod smart;

pub use model::{
    Album, LibraryStats, RemoteSyncReport, RemoteTrack, ScanFileError, ScanProgress, ScanReport,
    Track, TrackFilter,
};
pub use remote::{remote_id_from_path, remote_path};
pub use scanner::{AUDIO_EXTENSIONS, album_key};
pub use smart::{
    Field, MatchMode, Op, RuleGroup, RuleRow, RuleValue, SavedSmartView, SmartViewDef, SortField,
};

/// Name of the album-art cache directory (a sibling of the database file),
/// as derived by [`Library::open`]. The Tauri shell reuses this constant
/// so its `navinamp-art` scheme handler reads the exact same directory the
/// scanner writes.
pub const ART_CACHE_DIR: &str = art::CACHE_DIR;

use std::path::{Path, PathBuf};

use rusqlite::Connection;

/// Errors the library itself can produce. Per-file scanner failures are *not*
/// here — they are collected in [`ScanReport::errors`] so one broken file
/// never aborts a scan.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// A smart view definition failed validation (rule values, ops or
    /// limit) — or a stored definition was corrupt. The message names the
    /// offending field/op/rule index for inline UI display.
    #[error("invalid smart view: {0}")]
    InvalidSmartView(String),
    /// `smart_view_run`/`smart_view_delete` on an id that isn't saved.
    #[error("smart view {0} not found")]
    SmartViewNotFound(i64),
}

pub type Result<T> = std::result::Result<T, Error>;

/// An opened library database.
#[derive(Debug)]
pub struct Library {
    conn: Connection,
    /// `<db dir>/art-cache` — where the scanner caches album-art bytes
    /// (see `art.rs`). `None` for `:memory:` databases: no directory to
    /// cache in, so art extraction is disabled there.
    art_dir: Option<PathBuf>,
}

impl Library {
    /// Opens (creating if needed) the library at `path`, applies the schema
    /// and enables WAL + a 5 s busy timeout so multiple connections — e.g.
    /// the app's query connection and a scanning worker — coexist.
    ///
    /// The art cache is `<parent of path>/art-cache`, created on demand by
    /// the scanner.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        schema::init(&conn)?;
        // `:memory:` has no directory side; anything else caches art next
        // to the db file (a path with no parent directory — bare relative
        // names — has nowhere to cache either).
        let art_dir = if path.to_str() == Some(":memory:") {
            None
        } else {
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map(|parent| parent.join(art::CACHE_DIR))
        };
        Ok(Self { conn, art_dir })
    }

    /// The art-cache directory (sibling of the database file), or `None`
    /// for in-memory databases. The scanner writes here; the serving side
    /// (Tauri's `navinamp-art` scheme) reads here.
    pub fn art_dir(&self) -> Option<&Path> {
        self.art_dir.as_deref()
    }

    /// Scans `roots` incrementally. See the scanner docs / `README.md` for
    /// the full contract (upsert-by-mtime+size, root-scoped removal,
    /// per-file error tolerance, album-art extraction policy in `art.rs`).
    pub fn scan_roots(
        &self,
        roots: &[PathBuf],
        progress: impl FnMut(ScanProgress),
    ) -> Result<ScanReport> {
        Ok(scanner::scan_roots(
            &self.conn,
            self.art_dir.as_deref(),
            roots,
            progress,
        )?)
    }

    /// Upserts a full batch of remote (Navidrome) tracks and removes
    /// navidrome rows whose `remote_id` is absent from the batch — one
    /// transaction, local rows never touched. Callers should pass the
    /// *complete* walked library (not a delta); "remove unseen" only works
    /// that way. See `remote.rs` for the column semantics.
    pub fn sync_remote(&self, tracks: Vec<RemoteTrack>) -> Result<RemoteSyncReport> {
        Ok(remote::sync_remote(&self.conn, &tracks)?)
    }

    /// Ranked FTS5 search over title/artist/album/genre; each whitespace
    /// term is matched as a case-insensitive prefix (`beatl` → Beatles).
    pub fn search(&self, q: &str, limit: u32) -> Result<Vec<Track>> {
        Ok(query::search(&self.conn, q, limit)?)
    }

    /// All albums grouped by `album_key`, ordered artist → year → title.
    pub fn albums(&self) -> Result<Vec<Album>> {
        Ok(query::albums(&self.conn)?)
    }

    /// One album's tracks, in disc/track order.
    pub fn album_tracks(&self, album_key: &str) -> Result<Vec<Track>> {
        Ok(query::album_tracks(&self.conn, album_key)?)
    }

    /// All tracks of one artist (exact `artist` match).
    pub fn artist_tracks(&self, artist: &str) -> Result<Vec<Track>> {
        Ok(query::tracks_filtered(
            &self.conn,
            &TrackFilter {
                artist: Some(artist.to_owned()),
                ..TrackFilter::default()
            },
        )?)
    }

    /// All tracks of one genre (exact `genre` match).
    pub fn genre_tracks(&self, genre: &str) -> Result<Vec<Track>> {
        Ok(query::tracks_filtered(
            &self.conn,
            &TrackFilter {
                genre: Some(genre.to_owned()),
                ..TrackFilter::default()
            },
        )?)
    }

    /// Tracks matching any combination of artist / album / genre, paged.
    pub fn tracks_filtered(&self, filter: &TrackFilter) -> Result<Vec<Track>> {
        Ok(query::tracks_filtered(&self.conn, filter)?)
    }

    /// Library counters; `last_scan` is `None` until the first scan lands.
    pub fn stats(&self) -> Result<LibraryStats> {
        Ok(query::stats(&self.conn)?)
    }

    /// Row count, optionally scoped to one `source` (`local`/`navidrome`).
    pub fn track_count(&self, source: Option<&str>) -> Result<u64> {
        Ok(query::track_count(&self.conn, source)?)
    }

    /// Records one play of the local track at `path` (`play_count + 1`,
    /// `last_played_at = now`, unix seconds). Remote rows are never
    /// touched (their play stats are the server's business); an unknown
    /// path is a silent no-op.
    pub fn mark_played(&self, path: &str) -> Result<()> {
        Ok(query::mark_played(&self.conn, path)?)
    }

    /// Sets the 0–5 star rating of the track at `path` (both sources;
    /// values outside the range are clamped). An unknown path is a silent
    /// no-op.
    pub fn set_rating(&self, path: &str, rating: i64) -> Result<()> {
        Ok(query::set_rating(&self.conn, path, rating)?)
    }

    /// Mirrors the server's starred flag onto a navidrome row (called after
    /// a successful `star`/`unstar`). Local rows and unknown ids are no-ops.
    pub fn set_starred(&self, remote_id: &str, starred: bool) -> Result<()> {
        Ok(query::set_starred(&self.conn, remote_id, starred)?)
    }

    /// The stored art id for the track at `path`, if any — a local
    /// content hash (servable from the art cache) or, for `navidrome`
    /// rows, the server's `coverArt` id. Unknown paths answer `None`; the
    /// caller picks the serving route from the row's `source`.
    pub fn art_for_track(&self, path: &str) -> Result<Option<String>> {
        Ok(query::art_for_track(&self.conn, path)?)
    }

    // -- Smart Views (Winamp3-style smart playlists) -----------------------

    /// All saved smart views, in tree order (`position`, then id — i.e.
    /// creation order until drag-reorder lands).
    pub fn smart_views_list(&self) -> Result<Vec<SavedSmartView>> {
        smart::list_views(&self.conn)
    }

    /// Validates `def` and upserts it. `Some(id)` updates that row (or
    /// re-inserts under that id if it was deleted meanwhile); `None`
    /// appends. Returns the stored id. Nothing invalid is persisted.
    pub fn smart_view_save(&self, id: Option<i64>, def: &SmartViewDef) -> Result<i64> {
        smart::save_view(&self.conn, id, def)
    }

    /// Deletes a saved view; an unknown id is a no-op. Any remembered
    /// Navidrome publication mapping goes with it.
    pub fn smart_view_delete(&self, id: i64) -> Result<()> {
        smart::delete_view(&self.conn, id)
    }

    /// The Navidrome playlist a smart view was last published to, if any.
    pub fn smart_view_playlist_id(&self, id: i64) -> Result<Option<String>> {
        smart::remote_playlist_id(&self.conn, id)
    }

    /// Remembers the Navidrome playlist a smart view was published to (so
    /// republishing updates it instead of creating duplicates).
    pub fn set_smart_view_playlist_id(&self, id: i64, remote_playlist_id: &str) -> Result<()> {
        smart::set_remote_playlist_id(&self.conn, id, remote_playlist_id)
    }

    /// Runs a saved view by id. Fails with [`Error::SmartViewNotFound`]
    /// for an unknown id; `limit: null` in the def means unlimited rows.
    pub fn smart_view_run(&self, id: i64) -> Result<Vec<Track>> {
        smart::run_view(&self.conn, id)
    }

    /// Validates and evaluates `def` without persisting it. Hard cap of
    /// 500 rows (or the def's own smaller limit).
    pub fn smart_view_preview(&self, def: &SmartViewDef) -> Result<Vec<Track>> {
        smart::preview(&self.conn, def)
    }
}

// `Library` must stay `Send` so the bridge can hand it to worker threads;
// `Connection` is `Send` (rusqlite compiles bundled SQLite in serialized mode).
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<Library>();
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_in_memory_and_applies_schema() {
        // `:memory:` has no parent dir; make sure `open` handles that.
        let library = Library::open(":memory:").expect("open");
        let stats = library.stats().expect("stats");
        assert_eq!(stats, LibraryStats::default());
        assert!(library.albums().expect("albums").is_empty());
        assert!(library.search("anything", 10).expect("search").is_empty());
    }
}
