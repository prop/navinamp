//! Read-side queries: FTS search, album grouping, filtered track lists and
//! stats. All brief, single-statement affairs — the Tauri layer calls them
//! under a short-lived mutex.

use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Row};

use crate::model::{Album, LibraryStats, Track, TrackFilter};

/// Column list for every `SELECT` on `tracks`; positionally mapped by
/// [`track_from_row`]. With a prefix (`tracks.`) for joins where bare names
/// would be ambiguous (the FTS search join).
const TRACK_COLUMN_NAMES: &[&str] = &[
    "id",
    "path",
    "mtime",
    "size",
    "title",
    "artist",
    "album_artist",
    "album",
    "album_key",
    "genre",
    "year",
    "track_no",
    "disc_no",
    "duration_secs",
    "bitrate_kbps",
    "samplerate",
    "channels",
    "codec",
    "rating",
    "starred",
    "play_count",
    "last_played_at",
    "date_added",
    "source",
    "remote_id",
    "art_id",
];

/// Column list for every `SELECT` on `tracks`; positionally mapped by
/// [`track_from_row`]. With a prefix (`tracks.`) for joins where bare names
/// would be ambiguous (the FTS search join). Shared with `smart.rs`.
pub(super) fn track_columns(prefix: &str) -> String {
    TRACK_COLUMN_NAMES
        .iter()
        .map(|name| format!("{prefix}{name}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Maps a `tracks` row (in [`track_columns`] order) to a [`Track`].
/// Shared with `smart.rs`.
pub(super) fn track_from_row(row: &Row<'_>) -> rusqlite::Result<Track> {
    Ok(Track {
        id: row.get(0)?,
        path: row.get(1)?,
        mtime: row.get(2)?,
        size: row.get(3)?,
        title: row.get(4)?,
        artist: row.get(5)?,
        album_artist: row.get(6)?,
        album: row.get(7)?,
        album_key: row.get(8)?,
        genre: row.get(9)?,
        year: row.get(10)?,
        track_no: row.get(11)?,
        disc_no: row.get(12)?,
        duration_secs: row.get(13)?,
        bitrate_kbps: row.get(14)?,
        samplerate: row.get(15)?,
        channels: row.get(16)?,
        codec: row.get(17)?,
        rating: row.get(18)?,
        starred: row.get(19)?,
        play_count: row.get(20)?,
        last_played_at: row.get(21)?,
        date_added: row.get(22)?,
        source: row.get(23)?,
        remote_id: row.get(24)?,
        art_id: row.get(25)?,
    })
}

/// Builds an FTS5 MATCH pattern from free user input: every whitespace-separated
/// term becomes a quoted prefix phrase (`beatl` → `"beatl"*`). Quoting makes
/// `-`, `+`, `AND`, colons and friends inert; doubling embedded quotes escapes
/// them, so arbitrary input can never produce malformed or injected MATCH
/// syntax. All terms are ANDed (implicit).
fn fts_query(q: &str) -> Option<String> {
    let mut terms = q
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"*", t.replace('"', "\"\"")));
    let first = terms.next()?;
    Some(terms.fold(first, |acc, t| acc + " " + &t))
}

/// Ranked FTS5 search over title/artist/album/genre.
///
/// `bm25` ascending puts the best matches first (FTS5's `bm25()` is
/// smaller-is-better); the text tie-breakers keep the order stable and
/// library-like for weakly differentiated matches.
pub(super) fn search(conn: &Connection, q: &str, limit: u32) -> rusqlite::Result<Vec<Track>> {
    let Some(match_expr) = fts_query(q) else {
        return Ok(Vec::new());
    };
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {}
         FROM tracks_fts
         JOIN tracks ON tracks.id = tracks_fts.rowid
         WHERE tracks_fts MATCH ?1
         ORDER BY bm25(tracks_fts), tracks.artist, tracks.album, tracks.track_no, tracks.title
         LIMIT ?2",
        track_columns("tracks.")
    ))?;
    let tracks = stmt
        .query_map(rusqlite::params![match_expr, limit], track_from_row)?
        .collect::<Result<_, _>>()?;
    Ok(tracks)
}

/// All albums, grouped by `album_key` (see [`crate::scanner::album_key`]).
///
/// `MIN` picks a deterministic representative where sloppily-tagged members
/// disagree; `album_key` guarantees they at least agree on the normalized
/// (album)artist + album. The two correlated subqueries fetch the album's
/// art — the first non-null `art_id` in disc/track order — paired with the
/// `source` of the very track that carries it, so callers know whether the
/// id is a local content hash or a Navidrome `coverArt` id.
pub(super) fn albums(conn: &Connection) -> rusqlite::Result<Vec<Album>> {
    let mut stmt = conn.prepare_cached(
        "SELECT
             t.album_key,
             MIN(t.album)                                        AS name,
             MIN(COALESCE(NULLIF(t.album_artist, ''), NULLIF(t.artist, ''), '')) AS artist,
             MIN(t.year)                                         AS year,
             COUNT(*)                                            AS track_count,
             COALESCE(SUM(t.duration_secs), 0.0)                 AS total_secs,
             (SELECT t2.art_id FROM tracks t2
               WHERE t2.album_key = t.album_key AND t2.art_id IS NOT NULL
               ORDER BY t2.disc_no, t2.track_no, t2.path LIMIT 1),
             (SELECT t2.source FROM tracks t2
               WHERE t2.album_key = t.album_key AND t2.art_id IS NOT NULL
               ORDER BY t2.disc_no, t2.track_no, t2.path LIMIT 1)
         FROM tracks t
         GROUP BY t.album_key
         ORDER BY artist, year, name",
    )?;
    let albums = stmt
        .query_map([], |row| {
            Ok(Album {
                album_key: row.get(0)?,
                name: row.get(1)?,
                artist: row.get(2)?,
                year: row.get(3)?,
                track_count: row.get::<_, i64>(4)?.max(0) as u64,
                total_secs: row.get(5)?,
                art_id: row.get(6)?,
                art_source: row.get(7)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(albums)
}

/// One album's tracks in disc/track order (`path` as the final tie-breaker,
/// so track-number-less rips stay stable).
pub(super) fn album_tracks(conn: &Connection, album_key: &str) -> rusqlite::Result<Vec<Track>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {}
         FROM tracks
         WHERE album_key = ?1
         ORDER BY disc_no, track_no, path",
        track_columns("")
    ))?;
    let tracks = stmt
        .query_map([album_key], track_from_row)?
        .collect::<Result<_, _>>()?;
    Ok(tracks)
}

/// Tracks matching an optional artist / album / genre / source combination,
/// with limit + offset. `None` limit means unlimited (`LIMIT -1` in SQLite).
pub(super) fn tracks_filtered(
    conn: &Connection,
    filter: &TrackFilter,
) -> rusqlite::Result<Vec<Track>> {
    let mut sql = format!("SELECT {} FROM tracks", track_columns(""));
    let mut where_clauses: Vec<&'static str> = Vec::new();
    let mut params: Vec<&dyn rusqlite::ToSql> = Vec::new();

    if let Some(artist) = &filter.artist {
        where_clauses.push("artist = ?");
        params.push(artist);
    }
    if let Some(album_key) = &filter.album_key {
        where_clauses.push("album_key = ?");
        params.push(album_key);
    }
    if let Some(genre) = &filter.genre {
        where_clauses.push("genre = ?");
        params.push(genre);
    }
    if let Some(source) = &filter.source {
        where_clauses.push("source = ?");
        params.push(source);
    }
    if !where_clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&where_clauses.join(" AND "));
    }
    sql.push_str(" ORDER BY album_key, disc_no, track_no, path LIMIT ? OFFSET ?");
    let limit: i64 = match filter.limit {
        Some(n) => i64::from(n),
        None => -1,
    };
    params.push(&limit);
    let offset: i64 = i64::from(filter.offset);
    params.push(&offset);

    let mut stmt = conn.prepare(&sql)?;
    let tracks = stmt
        .query_map(params.as_slice(), track_from_row)?
        .collect::<Result<_, _>>()?;
    Ok(tracks)
}

/// Row count, optionally scoped to one `source` (`local`/`navidrome`).
/// Cheaper than `tracks_filtered` when only a number is needed (status
/// bars, sync reports).
pub(super) fn track_count(conn: &Connection, source: Option<&str>) -> rusqlite::Result<u64> {
    let n: i64 = match source {
        Some(s) => conn.query_row("SELECT COUNT(*) FROM tracks WHERE source = ?1", [s], |r| {
            r.get(0)
        })?,
        None => conn.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))?,
    };
    Ok(n.max(0) as u64)
}

/// Records one play of the local track at `path`: `play_count + 1`,
/// `last_played_at = now` (unix seconds). **Local rows only** — remote
/// play stats belong to the Navidrome server (scrobble), so a row with the
/// same path but `source != 'local'` is never touched. An unknown path is
/// a silent no-op (0 affected rows is not an error). One statement; FTS is
/// untouched in spirit (the update trigger re-inserts identical text,
/// which keeps the index correct without any real churn).
pub(super) fn mark_played(conn: &Connection, path: &str) -> rusqlite::Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    conn.execute(
        "UPDATE tracks SET play_count = play_count + 1, last_played_at = ?2
         WHERE path = ?1 AND source = 'local'",
        rusqlite::params![path, now],
    )?;
    Ok(())
}

/// Sets the 0–5 star rating for the track at `path` (local or navidrome
/// row; the value is clamped server-side). An unknown path is a silent
/// no-op (0 affected rows is not an error). Ratings live in the local DB
/// for both sources; the bridge pushes remote rows to the Subsonic server
/// separately when a connection exists.
pub(super) fn set_rating(conn: &Connection, path: &str, rating: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE tracks SET rating = ?2 WHERE path = ?1",
        rusqlite::params![path, rating.clamp(0, 5)],
    )?;
    Ok(())
}

/// Mirrors the server's starred flag onto a navidrome row. Local rows and
/// unknown ids are a silent no-op (0 affected rows is not an error).
pub(super) fn set_starred(
    conn: &Connection,
    remote_id: &str,
    starred: bool,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE tracks SET starred = ?2 WHERE source = 'navidrome' AND remote_id = ?1",
        rusqlite::params![remote_id, starred],
    )?;
    Ok(())
}

/// One track by its stored `path` (local path or `navidrome://id`), if it
/// exists. Used by the context menus to show the current star/rating state.
pub(super) fn track_by_path(conn: &Connection, path: &str) -> rusqlite::Result<Option<Track>> {
    conn.query_row(
        &format!("SELECT {} FROM tracks WHERE path = ?1", track_columns("")),
        [path],
        track_from_row,
    )
    .optional()
}

/// The stored art id of the track at `path`, if any (local content hash
/// or Navidrome `coverArt` id — the caller disambiguates via the row's
/// `source`). Unknown paths answer `None`; this is a point lookup on the
/// unique `path` column.
pub(super) fn art_for_track(conn: &Connection, path: &str) -> rusqlite::Result<Option<String>> {
    let art: Option<Option<String>> = conn
        .query_row("SELECT art_id FROM tracks WHERE path = ?1", [path], |row| {
            row.get(0)
        })
        .optional()?;
    Ok(art.flatten())
}

/// Whole-library counters; `last_scan` comes from the `meta` table.
pub(super) fn stats(conn: &Connection) -> rusqlite::Result<LibraryStats> {
    let mut stats = conn.query_row(
        "SELECT
             COUNT(*),
             COUNT(DISTINCT album_key),
             COUNT(DISTINCT COALESCE(NULLIF(album_artist, ''), artist)),
             COALESCE(SUM(duration_secs), 0.0)
         FROM tracks",
        [],
        |row| {
            Ok(LibraryStats {
                tracks: row.get::<_, i64>(0)?.max(0) as u64,
                albums: row.get::<_, i64>(1)?.max(0) as u64,
                artists: row.get::<_, i64>(2)?.max(0) as u64,
                total_secs: row.get(3)?,
                last_scan: None,
            })
        },
    )?;
    stats.last_scan = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'last_scan_at'",
            [],
            |row| {
                let value: String = row.get(0)?;
                Ok(value.parse::<i64>().ok())
            },
        )
        .optional()?
        .flatten();
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts_terms_are_quoted_prefixed_and_escaped() {
        assert_eq!(fts_query(""), None);
        assert_eq!(fts_query("   "), None);
        assert_eq!(fts_query("beatl").as_deref(), Some("\"beatl\"*"));
        // multiple terms AND together
        assert_eq!(
            fts_query("beat  let").as_deref(),
            Some("\"beat\"* \"let\"*")
        );
        // FTS operators are inert inside quotes; embedded quotes are doubled
        assert_eq!(
            fts_query("OR -x AND").as_deref(),
            Some("\"OR\"* \"-x\"* \"AND\"*")
        );
        assert_eq!(fts_query("a\"b").as_deref(), Some("\"a\"\"b\"*"));
    }
}
