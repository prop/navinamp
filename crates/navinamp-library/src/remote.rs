//! Remote (Navidrome) library sync: upsert a batch of [`RemoteTrack`]s into
//! `tracks` in one transaction, remove navidrome rows whose id wasn't seen.
//!
//! Semantics mirror the local scanner (see `scanner.rs`) so both sources
//! behave identically downstream:
//! - rows are keyed by `path` (`navidrome://<remote_id>`, unique, and never
//!   a filesystem path — the namespaces cannot collide);
//! - an upsert refreshes only sync-owned columns; user data (`play_count`,
//!   `last_played_at`) and provenance (`date_added`) survive. Server ratings
//!   seed rows still unrated locally (0), but a rating set in the app always
//!   wins over the server's copy;
//! - removal is scoped to `source = 'navidrome'` — **local rows are never
//!   touched**, not even read for deletion candidates by path;
//! - the whole batch lands in one transaction: a sync is atomic.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension};

use crate::model::{RemoteSyncReport, RemoteTrack};
use crate::scanner::album_key;

/// `path` for a navidrome row — the scheme-prefixed remote id. Public so
/// the Tauri bridge can hand a playing track's `path` back to the stream /
/// scrobble commands (they strip the prefix to recover the remote id).
pub fn remote_path(remote_id: &str) -> String {
    format!("navidrome://{remote_id}")
}

/// Strips the `navidrome://` prefix from `path`, if it carries one.
pub fn remote_id_from_path(path: &str) -> Option<&str> {
    path.strip_prefix("navidrome://")
}

/// Upserts `tracks` (source `navidrome`) and removes navidrome rows whose
/// `remote_id` is absent from the batch. See the module docs for the exact
/// semantics; `frozen` is the caller's report.
pub(super) fn sync_remote(
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
            // Defensive: an empty id would produce `navidrome://`, which
            // could never be played or removed again.
            if t.remote_id.is_empty() {
                continue;
            }
            let path = remote_path(&t.remote_id);
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
                now_secs,
                t.remote_id,
                t.art_id,
            ])?;

            if was_new {
                report.added += 1;
            } else {
                report.updated += 1;
            }
        }

        // Removal, scoped strictly to navidrome rows: collect the batch's
        // ids, then delete rows of this source whose id is missing. Local
        // rows are not considered candidates at all.
        let mut seen: HashSet<&str> = HashSet::with_capacity(tracks.len());
        for t in tracks {
            if !t.remote_id.is_empty() {
                seen.insert(t.remote_id.as_str());
            }
        }
        {
            let mut stmt =
                tx.prepare("SELECT id, remote_id FROM tracks WHERE source = 'navidrome'")?;
            let candidates: Vec<(i64, Option<String>)> = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<_, _>>()?;
            drop(stmt);
            let mut delete = tx.prepare("DELETE FROM tracks WHERE id = ?1")?;
            for (id, remote_id) in candidates {
                let keep = remote_id.as_deref().is_some_and(|rid| seen.contains(rid));
                if !keep {
                    delete.execute([id])?;
                    report.removed += 1;
                }
            }
        }
    }
    tx.commit()?;
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
    channels, codec, rating, date_added, source, remote_id, art_id
) VALUES (
    ?1, 0, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, 'navidrome', ?19, ?20
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
    remote_id       = excluded.remote_id,
    art_id          = excluded.art_id";
