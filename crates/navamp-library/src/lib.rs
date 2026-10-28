//! NavAmp music library: an incremental file scanner over a single SQLite
//! database with an FTS5 search index.
//!
//! The crate is deliberately free of Tauri/app concerns: give it a database
//! path (or `:memory:`), scan some directories, query what you got. The
//! Tauri shell in `src-tauri/src/library.rs` is a thin wrapper.
//!
//! ```no_run
//! use std::path::PathBuf;
//! use navamp_library::Library;
//!
//! let library = Library::open("/somewhere/navamp.db").unwrap();
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

mod model;
mod query;
mod scanner;
mod schema;
mod smart;

pub use model::{Album, LibraryStats, ScanFileError, ScanProgress, ScanReport, Track, TrackFilter};
pub use scanner::{AUDIO_EXTENSIONS, album_key};
pub use smart::{
    Field, MatchMode, Op, RuleGroup, RuleRow, RuleValue, SavedSmartView, SmartViewDef, SortField,
};

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
}

impl Library {
    /// Opens (creating if needed) the library at `path`, applies the schema
    /// and enables WAL + a 5 s busy timeout so multiple connections — e.g.
    /// the app's query connection and a scanning worker — coexist.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        schema::init(&conn)?;
        Ok(Self { conn })
    }

    /// Scans `roots` incrementally. See the scanner docs / `README.md` for
    /// the full contract (upsert-by-mtime+size, root-scoped removal,
    /// per-file error tolerance).
    pub fn scan_roots(
        &self,
        roots: &[PathBuf],
        progress: impl FnMut(ScanProgress),
    ) -> Result<ScanReport> {
        Ok(scanner::scan_roots(&self.conn, roots, progress)?)
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

    /// Deletes a saved view; an unknown id is a no-op.
    pub fn smart_view_delete(&self, id: i64) -> Result<()> {
        smart::delete_view(&self.conn, id)
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
