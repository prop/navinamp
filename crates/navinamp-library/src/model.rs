//! Crate-wide types that cross the FFI/IPC boundary and the scanner's
//! progress/report shapes. Everything serde-serializable uses `camelCase`
//! so it maps straight onto the TypeScript side of the Tauri bridge.

use serde::{Deserialize, Serialize};

/// One row of the `tracks` table, as returned by every query.
///
/// `path` is stored lossily (see the schema docs in `README.md`); everything
/// text-based is `""` rather than `NULL` when a tag is missing, so consumers
/// never fight `Option<String>` for display strings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: i64,
    /// Absolute path as stored in the DB (lossy string).
    pub path: String,
    /// File mtime in Unix **milliseconds** — fine-grained enough for
    /// same-second rewrites, which second-granularity scanning would miss.
    pub mtime: i64,
    pub size: i64,
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    /// Grouping key `(album_artist || artist)\x00album`, lower-cased and
    /// trimmed — see [`crate::scanner::album_key`].
    pub album_key: String,
    pub genre: String,
    pub year: Option<i64>,
    pub track_no: Option<i64>,
    pub disc_no: Option<i64>,
    pub duration_secs: f64,
    pub bitrate_kbps: Option<i64>,
    pub samplerate: Option<i64>,
    pub channels: Option<i64>,
    /// Short container/codec id: `mp3`, `flac`, `ogg`, `opus`, `m4a`, …
    pub codec: String,
    pub rating: i64,
    /// Favorited on the server (`starred`); always false for local rows.
    pub starred: bool,
    pub play_count: i64,
    /// Unix seconds; `None` = never played.
    pub last_played_at: Option<i64>,
    /// Unix seconds, set when the track was first inserted.
    pub date_added: i64,
    /// `local` today; `navidrome` rows arrive with the Navidrome sync (M5).
    pub source: String,
    pub remote_id: Option<String>,
    /// Album-art reference. For `local` rows: a 16-hex content hash into
    /// the art cache (`<db dir>/art-cache/<art_id>.<ext>`, served by the
    /// app's `navinamp-art` scheme). For `navidrome` rows: the server's
    /// `coverArt` id (feed it to the Subsonic `getCoverArt` URL builder).
    /// `None` = no art found.
    pub art_id: Option<String>,
}

/// An album as a grouped view over `tracks.album_key`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub album_key: String,
    /// Display name (one representative of the group).
    pub name: String,
    /// Album artist, falling back to the track artist.
    pub artist: String,
    /// Earliest year found among the album's tracks, if any.
    pub year: Option<i64>,
    pub track_count: u64,
    pub total_secs: f64,
    /// First non-null `art_id` among the album's tracks (disc/track order)
    /// — same semantics as [`Track::art_id`]. `None` when no track has art.
    pub art_id: Option<String>,
    /// `source` of the track that provided `art_id` (`local` |
    /// `navidrome`), telling the frontend which serving route to use.
    /// `None` exactly when `art_id` is.
    pub art_source: Option<String>,
}

/// Per-source counters (the library's source chips and tree counts).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceStats {
    pub tracks: u64,
    pub albums: u64,
    pub artists: u64,
    pub genres: u64,
    pub total_secs: f64,
}

/// Whole-library counters for the status bar / empty-state UI.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryStats {
    pub tracks: u64,
    pub albums: u64,
    pub artists: u64,
    pub genres: u64,
    pub total_secs: f64,
    /// Unix **milliseconds** of the last completed scan, if any.
    pub last_scan: Option<i64>,
    /// Rows whose `source = 'local'`.
    pub local: SourceStats,
    /// Rows whose `source = 'navidrome'`.
    pub navidrome: SourceStats,
}

/// Optional constraints for [`crate::Library::tracks_filtered`]. All fields
/// optional; `None` limits mean "no limit".
#[derive(Debug, Clone, Default)]
pub struct TrackFilter {
    /// Exact artist match (case-sensitive, against `tracks.artist`).
    pub artist: Option<String>,
    /// Exact album grouping key (from [`Album::album_key`]).
    pub album_key: Option<String>,
    /// Exact genre match.
    pub genre: Option<String>,
    /// Exact source match (`local` | `navidrome`); `None` = both.
    pub source: Option<String>,
    /// Max rows; `None` = unlimited.
    pub limit: Option<u32>,
    /// Rows to skip (0 by default).
    pub offset: u32,
}

/// Live progress from a running scan. Emitted through the Tauri bridge as
/// `library-progress` events.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ScanProgress {
    /// The discovery walk finished; `total` is the number of candidate audio
    /// files about to be processed.
    Discovered { total: u64 },
    /// One more file processed (`scanned` counts up to `total`).
    Progress { scanned: u64, total: u64 },
}

/// Why a single file could not be indexed. Never fatal — the scan continues.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanFileError {
    pub path: String,
    pub error: String,
}

/// Everything the caller needs to know about one completed scan.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    /// Canonical scan roots actually used (roots that could not be read are
    /// reported in `errors` and dropped here).
    pub roots: Vec<String>,
    /// Audio-extension files discovered under the roots (after de-duping
    /// overlapping roots).
    pub files_found: u64,
    pub added: u64,
    pub updated: u64,
    pub unchanged: u64,
    /// Rows removed because their files vanished **under the scanned roots**.
    pub removed: u64,
    pub errors: Vec<ScanFileError>,
    /// Wall-clock duration of the scan.
    pub duration_ms: u64,
}

/// One remote (Navidrome) track as delivered by a sync — everything the
/// sync owns, no file I/O involved. `path` is derived as
/// `navidrome://<remote_id>`; `album_key` is computed by the library.
///
/// Mapping from Subsonic's `Child` happens in the caller (the Tauri
/// bridge): `album_artist` should come from the enclosing album when the
/// song lacks one, `codec` from the `suffix`, `duration_secs` from
/// `song.duration`, `art_id` from `coverArt`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteTrack {
    /// Subsonic id of the song (also the stable identity across syncs).
    pub remote_id: String,
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub genre: String,
    pub year: Option<i64>,
    pub track_no: Option<i64>,
    pub disc_no: Option<i64>,
    /// Subsonic duration is whole seconds.
    pub duration_secs: f64,
    pub bitrate_kbps: Option<i64>,
    pub samplerate: Option<i64>,
    pub channels: Option<i64>,
    /// Container/codec id from the suffix (`mp3`, `flac`, …).
    pub codec: String,
    /// The server's user rating (0 = unrated, 1–5 stars). Imported into
    /// unrated local rows; a rating set in NaVinaMP is never overwritten.
    pub rating: i64,
    /// Favorited on the server (`starred`). Server-owned: every sync
    /// refreshes it.
    pub starred: bool,
    /// Subsonic `coverArt` id, if the server exposes one.
    pub art_id: Option<String>,
    /// File size in bytes when the server reports it.
    pub size: Option<i64>,
}

/// Counters from one [`crate::Library::sync_remote`] transaction. The
/// Tauri layer folds these into its user-facing sync report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSyncReport {
    pub added: u64,
    pub updated: u64,
    /// Navidrome rows whose `remote_id` was not part of the batch.
    pub removed: u64,
}
