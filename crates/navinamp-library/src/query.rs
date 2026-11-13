//! Read-side queries: FTS search, album grouping, filtered track lists and
//! stats. All brief, single-statement affairs — the Tauri layer calls them
//! under a short-lived mutex.

use std::cmp::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Row};

use crate::model::{Album, LibraryStats, ServerStats, SourceStats, Track, TrackFilter};

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
    "server_id",
    "remote_path",
    "art_id",
    "label",
    "catalog_num",
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
        server_id: row.get(25)?,
        remote_path: row.get(26)?,
        art_id: row.get(27)?,
        label: row.get(28)?,
        catalog_number: row.get(29)?,
    })
}

/// Case-insensitive natural-order comparison for file paths: digit runs are
/// compared by value, so `2` sorts before `10` even when the file name is not
/// zero-padded. Registered as the `PATH_NATURAL` SQLite collation by
/// [`crate::Library::open`]; the paged flat list uses it as its primary key.
pub(crate) fn natural_path_cmp(a: &str, b: &str) -> Ordering {
    let a = a.as_bytes();
    let b = b.as_bytes();
    let (mut i, mut j) = (0usize, 0usize);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let ai = digit_end(a, i);
            let bj = digit_end(b, j);
            let an = strip_leading_zeros(&a[i..ai]);
            let bn = strip_leading_zeros(&b[j..bj]);
            let ord = an.len().cmp(&bn.len()).then_with(|| an.cmp(bn));
            if ord != Ordering::Equal {
                return ord;
            }
            i = ai;
            j = bj;
            continue;
        }
        let ca = a[i].to_ascii_lowercase();
        let cb = b[j].to_ascii_lowercase();
        if ca != cb {
            return ca.cmp(&cb);
        }
        i += 1;
        j += 1;
    }
    (a.len() - i).cmp(&(b.len() - j))
}

fn digit_end(bytes: &[u8], mut at: usize) -> usize {
    while at < bytes.len() && bytes[at].is_ascii_digit() {
        at += 1;
    }
    at
}

fn strip_leading_zeros(mut bytes: &[u8]) -> &[u8] {
    while bytes.len() > 1 && bytes[0] == b'0' {
        bytes = &bytes[1..];
    }
    bytes
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

/// Ranked FTS5 search over title/artist/album/genre plus album_artist/path.
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
pub(super) fn albums(conn: &Connection, source: Option<&str>) -> rusqlite::Result<Vec<Album>> {
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
               ORDER BY t2.disc_no, t2.track_no, t2.path LIMIT 1),
             (SELECT t2.server_id FROM tracks t2
               WHERE t2.album_key = t.album_key AND t2.art_id IS NOT NULL
               ORDER BY t2.disc_no, t2.track_no, t2.path LIMIT 1),
             -- One representative pair, so a label and its catalog number
             -- always come from the same remote_albums row. MAX prefers a
             -- real pair over the empty marker produced by a missing join.
             MAX(COALESCE(ra.label, '') || char(31) || COALESCE(ra.catalog_num, ''))
                                                                AS label_pair
         FROM tracks t
         LEFT JOIN remote_albums ra
             ON ra.server_id = t.server_id AND ra.album_id = t.remote_album_id
         WHERE ?1 IS NULL OR t.source = ?1
         GROUP BY t.album_key
         ORDER BY artist, year, name",
    )?;
    let albums = stmt
        .query_map([source], |row| {
            let (label, catalog_number) = split_label_pair(row.get(9)?);
            Ok(Album {
                album_key: row.get(0)?,
                name: row.get(1)?,
                artist: row.get(2)?,
                year: row.get(3)?,
                track_count: row.get::<_, i64>(4)?.max(0) as u64,
                total_secs: row.get(5)?,
                art_id: row.get(6)?,
                art_source: row.get(7)?,
                art_server_id: row.get(8)?,
                label,
                catalog_number,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(albums)
}

/// Split the `label␟catalog` pair selected by `albums()` (char(31) separator).
/// Empty halves become `None`, so missing metadata never renders as blank.
fn split_label_pair(pair: Option<String>) -> (Option<String>, Option<String>) {
    let Some(pair) = pair else {
        return (None, None);
    };
    let mut parts = pair.splitn(2, '\u{1f}');
    let label = parts.next().filter(|s| !s.is_empty()).map(str::to_owned);
    let catalog = parts.next().filter(|s| !s.is_empty()).map(str::to_owned);
    (label, catalog)
}

/// Distinct track artists, optionally scoped to one source, sorted
/// case-insensitively (SQLite's ASCII fold, matching the smart-view rules).
pub(super) fn artists(conn: &Connection, source: Option<&str>) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT artist FROM tracks
         WHERE artist != '' AND (?1 IS NULL OR source = ?1)
         ORDER BY artist COLLATE NOCASE, artist",
    )?;
    let rows = stmt.query_map([source], |row| row.get(0))?;
    rows.collect()
}

/// Distinct non-empty genres, optionally scoped to one source.
pub(super) fn genres(conn: &Connection, source: Option<&str>) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT genre FROM tracks
         WHERE genre != '' AND (?1 IS NULL OR source = ?1)
         ORDER BY genre COLLATE NOCASE, genre",
    )?;
    let rows = stmt.query_map([source], |row| row.get(0))?;
    rows.collect()
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
///
/// Ordered by the real file path first (`remote_path` for Navidrome rows,
/// their filesystem path for local rows), then album/disc/track as fallbacks.
/// This is also the order used by bulk ENQUEUE/PLAY ALL.
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
    if let Some(server_id) = &filter.server_id {
        where_clauses.push("server_id = ?");
        params.push(server_id);
    }
    if !where_clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&where_clauses.join(" AND "));
    }
    // Path first: remote rows use the server-relative `remote_path` (the
    // synthetic `navidrome://server/id` path would sort by random id), while
    // local rows use their filesystem path. Album/disc/track remain as
    // fallbacks for paths that do not encode the track order.
    sql.push_str(
        " ORDER BY COALESCE(NULLIF(remote_path, ''), path) COLLATE PATH_NATURAL,
                   album_key, disc_no, track_no, path
           LIMIT ? OFFSET ?",
    );
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

/// Row count for one Navidrome profile (the per-server tree leaves).
pub(super) fn track_count_for_server(conn: &Connection, server_id: &str) -> rusqlite::Result<u64> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tracks WHERE source = 'navidrome' AND server_id = ?1",
        [server_id],
        |r| r.get(0),
    )?;
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

/// Mirrors the server's starred flag onto a navidrome row. Scoped by
/// server + remote id (ids are only unique per server). Local rows and
/// unknown ids are a silent no-op.
pub(super) fn set_starred(
    conn: &Connection,
    server_id: &str,
    remote_id: &str,
    starred: bool,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE tracks SET starred = ?3
         WHERE source = 'navidrome' AND server_id = ?1 AND remote_id = ?2",
        rusqlite::params![server_id, remote_id, starred],
    )?;
    Ok(())
}

/// One track by its stored `path` (local path or `navidrome://server/id`), if it
/// exists. Used by the context menus to show the current star/rating state.
pub(super) fn track_by_path(conn: &Connection, path: &str) -> rusqlite::Result<Option<Track>> {
    let mut track = conn
        .query_row(
            &format!("SELECT {} FROM tracks WHERE path = ?1", track_columns("")),
            [path],
            track_from_row,
        )
        .optional()?;
    // The album join supplements rows whose own tag columns are NULL; bulk
    // track queries stay join-free.
    if let Some(track) = track.as_mut() {
        let pair: Option<String> = conn.query_row(
            "SELECT MAX(COALESCE(ra.label, '') || char(31) || COALESCE(ra.catalog_num, ''))
               FROM tracks t
               LEFT JOIN remote_albums ra
                 ON ra.server_id = t.server_id AND ra.album_id = t.remote_album_id
              WHERE t.path = ?1",
            [path],
            |row| row.get(0),
        )?;
        // The row menu shows release metadata. The track's own synced
        // columns are authoritative (what the file carries); the album-level
        // join is only the fallback for rows without their own tag.
        let (album_label, album_catalog) = split_label_pair(pair);
        if track.label.is_none() {
            track.label = album_label;
        }
        if track.catalog_number.is_none() {
            track.catalog_number = album_catalog;
        }
    }
    Ok(track)
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
             COUNT(DISTINCT CASE WHEN genre != '' THEN genre END),
             COALESCE(SUM(duration_secs), 0.0)
         FROM tracks",
        [],
        |row| {
            Ok(LibraryStats {
                tracks: row.get::<_, i64>(0)?.max(0) as u64,
                albums: row.get::<_, i64>(1)?.max(0) as u64,
                artists: row.get::<_, i64>(2)?.max(0) as u64,
                genres: row.get::<_, i64>(3)?.max(0) as u64,
                total_secs: row.get(4)?,
                last_scan: None,
                local: SourceStats::default(),
                navidrome: SourceStats::default(),
                navidrome_servers: Vec::new(),
            })
        },
    )?;
    {
        let mut stmt = conn.prepare_cached(
            "SELECT
                 source,
                 COUNT(*),
                 COUNT(DISTINCT album_key),
                 COUNT(DISTINCT COALESCE(NULLIF(album_artist, ''), artist)),
                 COUNT(DISTINCT CASE WHEN genre != '' THEN genre END),
                 COALESCE(SUM(duration_secs), 0.0)
             FROM tracks
             GROUP BY source",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                SourceStats {
                    tracks: row.get::<_, i64>(1)?.max(0) as u64,
                    albums: row.get::<_, i64>(2)?.max(0) as u64,
                    artists: row.get::<_, i64>(3)?.max(0) as u64,
                    genres: row.get::<_, i64>(4)?.max(0) as u64,
                    total_secs: row.get(5)?,
                },
            ))
        })?;
        for row in rows {
            let (source, values) = row?;
            match source.as_str() {
                "local" => stats.local = values,
                "navidrome" => stats.navidrome = values,
                _ => {} // unknown future source: totals still include it
            }
        }
    }
    {
        let mut stmt = conn.prepare_cached(
            "SELECT
                 server_id,
                 COUNT(*),
                 COUNT(DISTINCT album_key),
                 COUNT(DISTINCT COALESCE(NULLIF(album_artist, ''), artist)),
                 COUNT(DISTINCT CASE WHEN genre != '' THEN genre END),
                 COALESCE(SUM(duration_secs), 0.0)
             FROM tracks
             WHERE source = 'navidrome' AND server_id IS NOT NULL
             GROUP BY server_id
             ORDER BY server_id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ServerStats {
                server_id: row.get(0)?,
                tracks: row.get::<_, i64>(1)?.max(0) as u64,
                albums: row.get::<_, i64>(2)?.max(0) as u64,
                artists: row.get::<_, i64>(3)?.max(0) as u64,
                genres: row.get::<_, i64>(4)?.max(0) as u64,
                total_secs: row.get(5)?,
            })
        })?;
        stats.navidrome_servers = rows.collect::<Result<_, _>>()?;
    }
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
    fn natural_path_cmp_orders_numbers_by_value() {
        assert_eq!(natural_path_cmp("1", "2"), Ordering::Less);
        assert_eq!(natural_path_cmp("2", "10"), Ordering::Less);
        assert_eq!(natural_path_cmp("10", "2"), Ordering::Greater);
        // Leading zeros do not change the numeric value.
        assert_eq!(natural_path_cmp("track02", "track2"), Ordering::Equal);
        // Case-insensitive ASCII, with the shorter prefix first.
        assert_eq!(natural_path_cmp("A/1", "a/2"), Ordering::Less);
        assert_eq!(natural_path_cmp("abc", "abcd"), Ordering::Less);
        assert_eq!(natural_path_cmp("same", "same"), Ordering::Equal);
    }

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
