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

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension};

use crate::model::{RemoteSyncReport, RemoteTrack};
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
) -> rusqlite::Result<RemoteSyncReport> {
    let mut report = RemoteSyncReport::default();

    let tx = conn.unchecked_transaction()?;
    {
        let mut lookup = tx.prepare("SELECT id FROM tracks WHERE path = ?1")?;
        let mut upsert = tx.prepare(REMOTE_UPSERT_SQL)?;
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
                t.art_id,
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

/// Deletes every remote row belonging to one server (profile removal).
pub(super) fn delete_server(conn: &Connection, server_id: &str) -> rusqlite::Result<u64> {
    let n = conn.execute(
        "DELETE FROM tracks WHERE source = 'navidrome' AND server_id = ?1",
        [server_id],
    )?;
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
) -> rusqlite::Result<RemoteSyncReport> {
    let mut report = upsert_remote(conn, tracks)?;
    let seen: HashSet<String> = tracks
        .iter()
        .filter(|t| !t.remote_id.is_empty() && t.server_id == server_id)
        .map(|t| t.remote_id.clone())
        .collect();
    report.removed = prune_remote(conn, server_id, &seen)?;
    Ok(report)
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
    channels, codec, rating, starred, date_added, source, remote_id, server_id, remote_path, art_id
) VALUES (
    ?1, 0, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, 'navidrome', ?20, ?21, ?22, ?23
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
    art_id          = excluded.art_id";
