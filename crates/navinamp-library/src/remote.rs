//! Remote (Navidrome) library sync: upsert a batch of [`RemoteTrack`]s into
//! `tracks` in one transaction, remove navidrome rows whose id wasn't seen.
//!
//! Semantics mirror the local scanner (see `scanner.rs`) so both sources
//! behave identically downstream:
//! - rows are keyed by `path` (`navidrome://<server_id>/<remote_id>`,
//!   unique across servers; remote ids are only unique per server);
//! - removal is scoped to `source = 'navidrome'` **and one server** —
//!   local rows and other servers are never touched;
//! - the whole batch lands in one transaction: a sync is atomic.
//!
//! `remote_albums` stores per-album fingerprints (song count, duration and
//! the cheap album-index metadata). A quick rescan compares the album index
//! against them and fetches only changed albums; [`remote_track_ids_by_album`]
//! supplies the untouched albums' ids for pruning.

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension};

use crate::model::{RemoteAlbum, RemoteSyncReport, RemoteTrack};
use crate::scanner::album_key;

/// `path` for a navidrome row: the server profile id plus the remote id.
/// Remote ids are only unique per server, so both are needed. Public so the
/// Tauri bridge can hand a playing track's `path` back to the stream /
/// scrobble commands (they strip the prefix to recover the pair).
pub fn remote_path(server_id: &str, remote_id: &str) -> String {
    format!("navidrome://{server_id}/{remote_id}")
}

/// Splits a remote path into `(server_id, remote_id)`. `None` for local
/// paths and for legacy `navidrome://<remote_id>` paths.
pub fn remote_ref_from_path(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix("navidrome://")?;
    let (server, remote) = rest.split_once('/')?;
    (!server.is_empty() && !remote.is_empty()).then_some((server, remote))
}

/// Upserts one batch of remote tracks, **without removals**. The sync walk
/// calls this incrementally so the library UI can render rows before the
/// whole server library has landed; call [`prune_remote`] with every id seen
/// once the walk completes.
pub(super) fn upsert_remote(
    conn: &Connection,
    tracks: &[RemoteTrack],
    preserve_missing_tags: bool,
) -> rusqlite::Result<RemoteSyncReport> {
    let sql = if preserve_missing_tags {
        REMOTE_UPSERT_PRESERVE_TAGS_SQL
    } else {
        REMOTE_UPSERT_SQL
    };
    let mut report = RemoteSyncReport::default();

    let tx = conn.unchecked_transaction()?;
    {
        let mut lookup = tx.prepare("SELECT id FROM tracks WHERE path = ?1")?;
        let mut upsert = tx.prepare(sql)?;
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        for t in tracks {
            // Defensive: empty identities would produce an unplayable path
            // that could never be removed again.
            if t.remote_id.is_empty() || t.server_id.is_empty() {
                continue;
            }
            let path = remote_path(&t.server_id, &t.remote_id);
            let was_new = lookup
                .query_row([&path], |_row| Ok(()))
                .optional()?
                .is_none();

            upsert.execute(rusqlite::params![
                path,
                t.size.unwrap_or(0),
                t.title,
                t.artist,
                t.album_artist,
                t.album,
                album_key(&t.album_artist, &t.artist, &t.album),
                t.genre,
                t.year,
                t.track_no,
                t.disc_no,
                t.duration_secs,
                t.bitrate_kbps,
                t.samplerate,
                t.channels,
                t.codec,
                t.rating.clamp(0, 5),
                t.starred,
                now_secs,
                t.remote_id,
                t.server_id,
                t.remote_path,
                t.remote_album_id,
                t.art_id,
                t.label,
                t.catalog_num,
            ])?;

            if was_new {
                report.added += 1;
            } else {
                report.updated += 1;
            }
        }
    }
    tx.commit()?;
    Ok(report)
}

/// Deletes one server's navidrome rows whose `remote_id` is absent from
/// `seen`. Scoped to `source = 'navidrome' AND server_id = ?` — local rows
/// and other servers are never candidates. Returns the number removed. Only
/// called after a *complete* sync walk, so a failed sync never prunes rows it
/// simply did not reach.
pub(super) fn prune_remote(
    conn: &Connection,
    server_id: &str,
    seen: &HashSet<String>,
) -> rusqlite::Result<u64> {
    let tx = conn.unchecked_transaction()?;
    let mut removed = 0u64;
    {
        let mut stmt = tx.prepare(
            "SELECT id, remote_id FROM tracks
             WHERE source = 'navidrome' AND server_id = ?1",
        )?;
        let candidates: Vec<(i64, Option<String>)> = stmt
            .query_map([server_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        drop(stmt);
        let mut delete = tx.prepare("DELETE FROM tracks WHERE id = ?1")?;
        for (id, remote_id) in candidates {
            let keep = remote_id.as_deref().is_some_and(|rid| seen.contains(rid));
            if !keep {
                delete.execute([id])?;
                removed += 1;
            }
        }
    }
    tx.commit()?;
    Ok(removed)
}

/// Load one server's album fingerprints, keyed by album id.
pub(super) fn remote_albums(
    conn: &Connection,
    server_id: &str,
) -> rusqlite::Result<HashMap<String, RemoteAlbum>> {
    let mut stmt = conn.prepare(
        "SELECT album_id, song_count, duration_secs, name, artist, year,
                cover_art_id, created, label, catalog_num
           FROM remote_albums WHERE server_id = ?1",
    )?;
    let rows = stmt.query_map([server_id], |row| {
        Ok(RemoteAlbum {
            server_id: server_id.to_owned(),
            album_id: row.get(0)?,
            song_count: row.get::<_, i64>(1)?.max(0) as u32,
            duration_secs: row.get::<_, i64>(2)?.max(0) as u64,
            name: row.get(3)?,
            artist: row.get(4)?,
            year: row.get(5)?,
            cover_art_id: row.get(6)?,
            created: row.get(7)?,
            label: row.get(8)?,
            catalog_num: row.get(9)?,
        })
    })?;
    let mut albums = HashMap::new();
    for album in rows {
        let album = album?;
        albums.insert(album.album_id.clone(), album);
    }
    Ok(albums)
}

/// Remote track ids grouped by album id for one server. Quick rescans use
/// this to mark an unchanged album's tracks as seen without fetching it.
pub(super) fn remote_track_ids_by_album(
    conn: &Connection,
    server_id: &str,
) -> rusqlite::Result<HashMap<String, Vec<String>>> {
    let mut stmt = conn.prepare(
        "SELECT remote_album_id, remote_id FROM tracks
          WHERE source = 'navidrome' AND server_id = ?1
            AND remote_album_id IS NOT NULL AND remote_id IS NOT NULL",
    )?;
    let rows = stmt.query_map([server_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut by_album: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        let (album_id, remote_id) = row?;
        by_album.entry(album_id).or_default().push(remote_id);
    }
    Ok(by_album)
}

/// Insert or refresh album fingerprints in one transaction.
pub(super) fn upsert_remote_albums(
    conn: &Connection,
    albums: &[RemoteAlbum],
) -> rusqlite::Result<()> {
    if albums.is_empty() {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    {
        let mut upsert = tx.prepare(
            "INSERT INTO remote_albums (
                server_id, album_id, song_count, duration_secs, name, artist,
                year, cover_art_id, created, label, catalog_num
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(server_id, album_id) DO UPDATE SET
                song_count    = excluded.song_count,
                duration_secs = excluded.duration_secs,
                name          = excluded.name,
                artist        = excluded.artist,
                year          = excluded.year,
                cover_art_id  = excluded.cover_art_id,
                created       = excluded.created,
                label         = excluded.label,
                catalog_num   = excluded.catalog_num",
        )?;
        for album in albums {
            upsert.execute(rusqlite::params![
                album.server_id,
                album.album_id,
                i64::from(album.song_count),
                i64::try_from(album.duration_secs).unwrap_or(i64::MAX),
                album.name,
                album.artist,
                album.year,
                album.cover_art_id,
                album.created,
                album.label,
                album.catalog_num,
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Delete one server's album fingerprints whose album id is absent from
/// `seen`. Only called after a complete, error-free album-index walk.
pub(super) fn prune_remote_albums(
    conn: &Connection,
    server_id: &str,
    seen: &HashSet<String>,
) -> rusqlite::Result<u64> {
    let tx = conn.unchecked_transaction()?;
    let mut removed = 0u64;
    {
        let mut stmt = tx.prepare("SELECT album_id FROM remote_albums WHERE server_id = ?1")?;
        let candidates: Vec<String> = stmt
            .query_map([server_id], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        drop(stmt);
        let mut delete =
            tx.prepare("DELETE FROM remote_albums WHERE server_id = ?1 AND album_id = ?2")?;
        for album_id in candidates {
            if !seen.contains(&album_id) {
                delete.execute(rusqlite::params![server_id, album_id])?;
                removed += 1;
            }
        }
    }
    tx.commit()?;
    Ok(removed)
}

/// Deletes every remote row belonging to one server (profile removal).
pub(super) fn delete_server(conn: &Connection, server_id: &str) -> rusqlite::Result<u64> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM remote_albums WHERE server_id = ?1",
        [server_id],
    )?;
    let n = tx.execute(
        "DELETE FROM tracks WHERE source = 'navidrome' AND server_id = ?1",
        [server_id],
    )?;
    tx.commit()?;
    Ok(n as u64)
}

/// Rewrites pre-aggregation rows (`navidrome://<remote_id>`, NULL server id)
/// to the new `navidrome://<server_id>/<remote_id>` identity. Idempotent:
/// rows already carrying a server-qualified path are left alone. Rows with a
/// different server id are never touched.
pub(super) fn migrate_legacy_server(conn: &Connection, server_id: &str) -> rusqlite::Result<u64> {
    let prefix = format!("navidrome://{server_id}/");
    let prefix_len = prefix.chars().count() as i64;
    Ok(conn.execute(
        "UPDATE OR IGNORE tracks
         SET server_id = ?1,
             path = ?2 || remote_id
         WHERE source = 'navidrome'
           AND remote_id IS NOT NULL
           AND (server_id IS NULL OR server_id = ?1)
           AND substr(path, 1, ?3) != ?2",
        rusqlite::params![server_id, prefix, prefix_len],
    )? as u64)
}

/// One-shot upsert + prune for one server (tests and callers that already
/// hold the full batch). The incremental sync path uses [`upsert_remote`]
/// per batch and [`prune_remote`] at the end.
pub(super) fn sync_remote(
    conn: &Connection,
    server_id: &str,
    tracks: &[RemoteTrack],
    preserve_missing_tags: bool,
) -> rusqlite::Result<RemoteSyncReport> {
    let mut report = upsert_remote(conn, tracks, preserve_missing_tags)?;
    let seen: HashSet<String> = tracks
        .iter()
        .filter(|t| !t.remote_id.is_empty() && t.server_id == server_id)
        .map(|t| t.remote_id.clone())
        .collect();
    report.removed = prune_remote(conn, server_id, &seen)?;
    Ok(report)
}

/// Bulk-apply native-API per-track tags (`(remote_id, catalog_num, label)`
/// triples) to one server's rows without re-mapping album details. A track
/// whose triple carries no value falls back to its album's stored values,
/// the same fallback `upsert_remote` uses — one transaction, other servers
/// and local rows never touched. Returns the number of rows updated.
pub(super) fn apply_native_tags(
    conn: &Connection,
    server_id: &str,
    tags: &[(String, Option<String>, Option<String>)],
) -> rusqlite::Result<u64> {
    let tx = conn.unchecked_transaction()?;
    let mut updated = 0u64;
    {
        let mut stmt = tx.prepare(
            "UPDATE tracks SET
                 label       = COALESCE(?2, (SELECT ra.label
                                                FROM remote_albums ra
                                                WHERE ra.server_id = tracks.server_id
                                                  AND ra.album_id  = tracks.remote_album_id)),
                 catalog_num = COALESCE(?3, (SELECT ra.catalog_num
                                             FROM remote_albums ra
                                             WHERE ra.server_id = tracks.server_id
                                               AND ra.album_id  = tracks.remote_album_id))
               WHERE source = 'navidrome' AND server_id = ?1 AND remote_id = ?4",
        )?;
        for (remote_id, catalog_num, label) in tags {
            // SQL positional order: ?2 = label, ?3 = catalog_num.
            updated +=
                stmt.execute(rusqlite::params![server_id, label, catalog_num, remote_id])? as u64;
        }
    }
    tx.commit()?;
    Ok(updated)
}

/// Insert-or-refresh. On conflict, everything the sync owns is refreshed
/// (including `art_id` — cover art ids change when the server re-indexes);
/// local `play_count` / `last_played_at` and provenance (`source` — fixed at
/// `navidrome` for fresh rows — and `date_added`) survive untouched. Ratings
/// are imported only into rows that are still unrated locally (0), so a
/// rating set in the app is never clobbered by a sync. `mtime` is pinned to
/// 0: remote rows have no filesystem stamp and the local scanner never
/// examines them.
const REMOTE_UPSERT_SQL: &str = "\
INSERT INTO tracks (
    path, mtime, size, title, artist, album_artist, album, album_key, genre,
    year, track_no, disc_no, duration_secs, bitrate_kbps, samplerate,
    channels, codec, rating, starred, date_added, source, remote_id, server_id, remote_path,
    remote_album_id, art_id, label, catalog_num
) VALUES (
    ?1, 0, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, 'navidrome', ?20, ?21, ?22, ?23, ?24, ?25, ?26
)
ON CONFLICT(path) DO UPDATE SET
    size         = excluded.size,
    title        = excluded.title,
    artist       = excluded.artist,
    album_artist = excluded.album_artist,
    album        = excluded.album,
    album_key    = excluded.album_key,
    genre        = excluded.genre,
    year         = excluded.year,
    track_no     = excluded.track_no,
    disc_no      = excluded.disc_no,
    duration_secs   = excluded.duration_secs,
    bitrate_kbps    = excluded.bitrate_kbps,
    samplerate      = excluded.samplerate,
    channels        = excluded.channels,
    codec           = excluded.codec,
    rating          = CASE WHEN tracks.rating = 0 THEN excluded.rating ELSE tracks.rating END,
    starred         = excluded.starred,
    remote_id       = excluded.remote_id,
    server_id       = excluded.server_id,
    remote_path     = excluded.remote_path,
    remote_album_id = excluded.remote_album_id,
    art_id          = excluded.art_id,
    label           = excluded.label,
    catalog_num     = excluded.catalog_num";

/// Same upsert, but used when the per-track label/catalog source was
/// unavailable this run: `COALESCE` keeps the previously synced values
/// instead of letting a NULL overwrite them. Only a run that actually saw
/// the native tag map may clear a tag (wholesale convergence).
const REMOTE_UPSERT_PRESERVE_TAGS_SQL: &str = "\
INSERT INTO tracks (
    path, mtime, size, title, artist, album_artist, album, album_key, genre,
    year, track_no, disc_no, duration_secs, bitrate_kbps, samplerate,
    channels, codec, rating, starred, date_added, source, remote_id, server_id, remote_path,
    remote_album_id, art_id, label, catalog_num
) VALUES (
    ?1, 0, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, 'navidrome', ?20, ?21, ?22, ?23, ?24, ?25, ?26
)
ON CONFLICT(path) DO UPDATE SET
    size         = excluded.size,
    title        = excluded.title,
    artist       = excluded.artist,
    album_artist = excluded.album_artist,
    album        = excluded.album,
    album_key    = excluded.album_key,
    genre        = excluded.genre,
    year         = excluded.year,
    track_no     = excluded.track_no,
    disc_no      = excluded.disc_no,
    duration_secs   = excluded.duration_secs,
    bitrate_kbps    = excluded.bitrate_kbps,
    samplerate      = excluded.samplerate,
    channels        = excluded.channels,
    codec           = excluded.codec,
    rating          = CASE WHEN tracks.rating = 0 THEN excluded.rating ELSE tracks.rating END,
    starred         = excluded.starred,
    remote_id       = excluded.remote_id,
    server_id       = excluded.server_id,
    remote_path     = excluded.remote_path,
    remote_album_id = excluded.remote_album_id,
    art_id          = excluded.art_id,
    label           = COALESCE(excluded.label, tracks.label),
    catalog_num     = COALESCE(excluded.catalog_num, tracks.catalog_num)";
