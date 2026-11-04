//! DDL + the single place schema knowledge lives.
//!
//! Design (documented at length in `README.md`):
//! - `tracks` is the **single** track table (local + future remote rows,
//!   `source` discriminates); no separate artist/album tables — albums are a
//!   `GROUP BY album_key` view, which keeps upserts trivial and the scanner
//!   append-only.
//! - `tracks_fts` is an FTS5 **external-content** table over
//!   `title/artist/album/genre`, kept in sync by triggers. External content
//!   (rather than contentless) so the indexed text lives only once, in
//!   `tracks`, and plain-SQL readers never see a second copy drift.
//! - `meta` is a tiny key/value table for scanner bookkeeping
//!   (`last_scan_at`); it holds no track data.
//! - `smart_views` stores Smart View definitions as JSON blobs (see
//!   `smart.rs`); the scanner never touches it.

use std::borrow::Cow;

use rusqlite::OptionalExtension;

use crate::text::repair_tag_text;

/// Applies the full schema, idempotently. Adds columns introduced after the
/// first release to databases created by older builds (`CREATE TABLE IF NOT
/// EXISTS` never alters an existing table).
pub(super) fn init(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)?;
    if !has_column(conn, "tracks", "starred")? {
        conn.execute(
            "ALTER TABLE tracks ADD COLUMN starred INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    if !has_column(conn, "tracks", "server_id")? {
        // Multi-server aggregation: remote rows now carry their profile id.
        conn.execute("ALTER TABLE tracks ADD COLUMN server_id TEXT", [])?;
    }
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_tracks_server ON tracks(server_id)",
        [],
    )?;
    // FTS over title/artist/album/genre originally; simple search now also
    // covers album_artist and the full path (navidrome://server/id). The FTS5
    // table is external-content, so widening it means dropping/recreating the
    // table + triggers and rebuilding the index once.
    let rebuild_fts = !has_column(conn, "tracks_fts", "album_artist")?;
    if rebuild_fts {
        conn.execute_batch(
            "DROP TRIGGER IF EXISTS tracks_fts_ins;
             DROP TRIGGER IF EXISTS tracks_fts_del;
             DROP TRIGGER IF EXISTS tracks_fts_upd;
             DROP TABLE IF EXISTS tracks_fts;",
        )?;
    }
    conn.execute_batch(FTS_SCHEMA)?;
    if rebuild_fts {
        conn.execute("INSERT INTO tracks_fts(tracks_fts) VALUES('rebuild')", [])?;
    }
    if !has_column(conn, "smart_view_playlists", "server")? {
        // Rebuild the single-server mapping table with a composite key;
        // existing rows survive under the legacy '' server key.
        conn.execute_batch(
            "ALTER TABLE smart_view_playlists RENAME TO smart_view_playlists_legacy;
             CREATE TABLE smart_view_playlists (
                 server             TEXT NOT NULL DEFAULT '',
                 smart_view_id      INTEGER NOT NULL REFERENCES smart_views(id) ON DELETE CASCADE,
                 remote_playlist_id TEXT NOT NULL,
                 PRIMARY KEY (server, smart_view_id)
             );
             INSERT INTO smart_view_playlists (server, smart_view_id, remote_playlist_id)
                 SELECT '', smart_view_id, remote_playlist_id FROM smart_view_playlists_legacy;
             DROP TABLE smart_view_playlists_legacy;",
        )?;
    }
    repair_legacy_tag_text(conn)?;
    Ok(())
}

/// One-shot repair of tag text stored before charset recovery existed:
/// legacy ID3v1 / ISO-8859-1 ID3v2 tags often carry CP1251 or KOI8-R bytes,
/// which arrive Latin-1-mapped (`Ïåñíÿ î ëþáâè`). The scanner and the
/// Navidrome mapper repair new reads (`text.rs`); this heals rows already in
/// the database, which an incremental rescan would skip (unchanged
/// mtime/size).
///
/// Marker-guarded via `meta` so it runs exactly once per database. v1 only
/// repaired local rows; v2 also repairs remote/Navidrome rows — servers
/// commonly expose legacy ID3 text through Subsonic, so the old "remote text
/// is always UTF-8" assumption left those rows garbled — and picks up the
/// CP1250-mapped / Latin-prefixed shapes the v1 heuristic missed. The FTS
/// update trigger keeps the search index in step with the rewrites.
fn repair_legacy_tag_text(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    const MARK: &str = "text_charset_repair_v2";
    let done = conn
        .query_row("SELECT 1 FROM meta WHERE key = ?1", [MARK], |_| Ok(()))
        .optional()?
        .is_some();
    if done {
        return Ok(());
    }

    struct Fixed {
        id: i64,
        title: String,
        artist: String,
        album_artist: String,
        album: String,
        album_key: String,
        genre: String,
    }

    // One transaction for the whole pass: a large library would otherwise
    // issue thousands of autocommit UPDATEs and look like a hang at startup.
    let tx = conn.unchecked_transaction()?;
    let mut fixed: Vec<Fixed> = Vec::new();
    {
        let mut select =
            tx.prepare("SELECT id, title, artist, album_artist, album, genre FROM tracks")?;
        let mut rows = select.query([])?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let raw_title: String = row.get(1)?;
            let raw_artist: String = row.get(2)?;
            let raw_album_artist: String = row.get(3)?;
            let raw_album: String = row.get(4)?;
            let raw_genre: String = row.get(5)?;
            let title = repair_tag_text(&raw_title);
            let artist = repair_tag_text(&raw_artist);
            let album_artist = repair_tag_text(&raw_album_artist);
            let album = repair_tag_text(&raw_album);
            let genre = repair_tag_text(&raw_genre);
            let changed = [&title, &artist, &album_artist, &album, &genre]
                .iter()
                .any(|c| matches!(c, Cow::Owned(_)));
            if !changed {
                continue;
            }
            fixed.push(Fixed {
                id,
                album_key: crate::scanner::album_key(&album_artist, &artist, &album),
                title: title.into_owned(),
                artist: artist.into_owned(),
                album_artist: album_artist.into_owned(),
                album: album.into_owned(),
                genre: genre.into_owned(),
            });
        }
    }

    if !fixed.is_empty() {
        eprintln!(
            "navinamp library: repairing legacy tag text in {} row(s)…",
            fixed.len()
        );
        let mut update = tx.prepare(
            "UPDATE tracks
                SET title = ?2, artist = ?3, album_artist = ?4, album = ?5,
                    album_key = ?6, genre = ?7
              WHERE id = ?1",
        )?;
        for f in &fixed {
            update.execute(rusqlite::params![
                f.id,
                f.title,
                f.artist,
                f.album_artist,
                f.album,
                f.album_key,
                f.genre
            ])?;
        }
        eprintln!(
            "navinamp library: repaired legacy tag text in {} row(s)",
            fixed.len()
        );
    }

    tx.execute(
        "INSERT INTO meta(key, value) VALUES(?1, 'done')
         ON CONFLICT(key) DO UPDATE SET value = 'done'",
        [MARK],
    )?;
    tx.commit()?;
    Ok(())
}

/// Whether `table` already has a column named `column`.
fn has_column(conn: &rusqlite::Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

const SCHEMA: &str = "\
BEGIN;
CREATE TABLE IF NOT EXISTS tracks (
    id              INTEGER PRIMARY KEY,
    path            TEXT UNIQUE NOT NULL,
    mtime           INTEGER NOT NULL,          -- unix millis
    size            INTEGER NOT NULL,          -- bytes
    title           TEXT NOT NULL DEFAULT '',
    artist          TEXT NOT NULL DEFAULT '',
    album_artist    TEXT NOT NULL DEFAULT '',
    album           TEXT NOT NULL DEFAULT '',
    album_key       TEXT NOT NULL,             -- (album_artist|artist) + NUL + album, normalized
    genre           TEXT NOT NULL DEFAULT '',
    year            INTEGER,
    track_no        INTEGER,
    disc_no         INTEGER,
    duration_secs   REAL NOT NULL DEFAULT 0.0,
    bitrate_kbps    INTEGER,
    samplerate      INTEGER,
    channels        INTEGER,
    codec           TEXT NOT NULL DEFAULT '',
    rating          INTEGER NOT NULL DEFAULT 0,
    starred         INTEGER NOT NULL DEFAULT 0,  -- favorited (navidrome rows)
    play_count      INTEGER NOT NULL DEFAULT 0,
    last_played_at  INTEGER,                   -- unix seconds
    date_added      INTEGER NOT NULL,          -- unix seconds
    source          TEXT NOT NULL DEFAULT 'local',
    remote_id       TEXT,
    server_id       TEXT,                      -- navidrome profile id (remote rows)
    art_id          TEXT                       -- local: content hash into art-cache; remote: coverArt id
);

CREATE INDEX IF NOT EXISTS idx_tracks_album_key ON tracks(album_key);
CREATE INDEX IF NOT EXISTS idx_tracks_artist    ON tracks(artist);
CREATE INDEX IF NOT EXISTS idx_tracks_genre     ON tracks(genre);
CREATE INDEX IF NOT EXISTS idx_tracks_artist_album ON tracks(artist, album);

-- Full-text index: see FTS_SCHEMA below (widened with album_artist/path by
-- the init migration, so its definition lives outside this batch).

CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Saved Smart Views (Winamp3 media-library smart playlists). `def` holds
-- the serialized SmartViewDef JSON and is the source of truth; `name` is
-- denormalized for tree display and `position` is reserved for a future
-- drag-reorder (list order is `position, id` — i.e. creation order today).
CREATE TABLE IF NOT EXISTS smart_views (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    name     TEXT NOT NULL,
    position INTEGER NOT NULL DEFAULT 0,
    def      TEXT NOT NULL
);

-- Smart views published to Navidrome: the server playlist id remembered per
-- server profile so republishing updates the existing playlist instead of
-- creating duplicates (and switching servers doesn't clobber other servers'
-- mappings). `server` is the profile id; legacy rows use ''.
CREATE TABLE IF NOT EXISTS smart_view_playlists (
    server             TEXT NOT NULL DEFAULT '',
    smart_view_id      INTEGER NOT NULL REFERENCES smart_views(id) ON DELETE CASCADE,
    remote_playlist_id TEXT NOT NULL,
    PRIMARY KEY (server, smart_view_id)
);
COMMIT;
";

/// Full-text index over the searchable text. `tracks` is the external content
/// table (text lives there once); the triggers keep the token index in lockstep
/// with every write path. Kept separate from [`SCHEMA`] so the init migration
/// can rebuild it when the column set changes.
const FTS_SCHEMA: &str = "\
CREATE VIRTUAL TABLE IF NOT EXISTS tracks_fts USING fts5(
    title, artist, album, genre, album_artist, path,
    content='tracks', content_rowid='id'
);

CREATE TRIGGER IF NOT EXISTS tracks_fts_ins AFTER INSERT ON tracks BEGIN
    INSERT INTO tracks_fts(rowid, title, artist, album, genre, album_artist, path)
    VALUES (new.id, new.title, new.artist, new.album, new.genre, new.album_artist, new.path);
END;

CREATE TRIGGER IF NOT EXISTS tracks_fts_del AFTER DELETE ON tracks BEGIN
    INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album, genre, album_artist, path)
    VALUES ('delete', old.id, old.title, old.artist, old.album, old.genre, old.album_artist, old.path);
END;

CREATE TRIGGER IF NOT EXISTS tracks_fts_upd AFTER UPDATE ON tracks BEGIN
    INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album, genre, album_artist, path)
    VALUES ('delete', old.id, old.title, old.artist, old.album, old.genre, old.album_artist, old.path);
    INSERT INTO tracks_fts(rowid, title, artist, album, genre, album_artist, path)
    VALUES (new.id, new.title, new.artist, new.album, new.genre, new.album_artist, new.path);
END;
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repairs_existing_mojibake_rows_once() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        init(&conn).unwrap();

        // Seed a row the way a pre-repair scanner stored it (CP1251 bytes
        // mapped through Latin-1), then clear the marker so the pass runs.
        // Artist and album differ and album_artist is empty: the shape where
        // a wrong album_key derivation would diverge from a fresh scan.
        let (bytes, _, _) = encoding_rs::WINDOWS_1251.encode("Песня о любви");
        let garbled_title: String = bytes.iter().map(|b| char::from(*b)).collect();
        let (bytes, _, _) = encoding_rs::WINDOWS_1251.encode("Исполнитель");
        let garbled_artist: String = bytes.iter().map(|b| char::from(*b)).collect();
        let (bytes, _, _) = encoding_rs::WINDOWS_1251.encode("Альбом");
        let garbled_album: String = bytes.iter().map(|b| char::from(*b)).collect();
        conn.execute(
            "INSERT INTO tracks
                (path, mtime, size, title, artist, album, album_key, genre, date_added)
             VALUES ('/a.mp3', 0, 0, ?1, ?2, ?3, '', '', 0)",
            rusqlite::params![garbled_title, garbled_artist, garbled_album],
        )
        .unwrap();
        // Remote/Navidrome rows get the same repair in v2: servers commonly
        // expose legacy ID3 text through Subsonic.
        let (bytes, _, _) = encoding_rs::WINDOWS_1251.encode("Удалённая песня");
        let garbled_remote: String = bytes.iter().map(|b| char::from(*b)).collect();
        conn.execute(
            "INSERT INTO tracks
                (path, mtime, size, title, artist, album, album_key, genre, date_added, source)
             VALUES ('navidrome://p1/x', 0, 0, ?1, ?2, '', '', '', 0, 'navidrome')",
            rusqlite::params![garbled_remote, garbled_artist],
        )
        .unwrap();
        conn.execute("DELETE FROM meta WHERE key = 'text_charset_repair_v2'", [])
            .unwrap();

        repair_legacy_tag_text(&conn).unwrap();

        let (title, artist, album, key): (String, String, String, String) = conn
            .query_row(
                "SELECT title, artist, album, album_key FROM tracks",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(title, "Песня о любви");
        assert_eq!(artist, "Исполнитель");
        assert_eq!(album, "Альбом");
        assert_eq!(
            key,
            crate::scanner::album_key("", "Исполнитель", "Альбом"),
            "healed album_key must match what a fresh scan derives"
        );

        let (remote_title, remote_artist): (String, String) = conn
            .query_row(
                "SELECT title, artist FROM tracks WHERE source = 'navidrome'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(remote_title, "Удалённая песня");
        assert_eq!(remote_artist, "Исполнитель");

        // Marker set: a later damaged row is not touched by a second run.
        conn.execute("UPDATE tracks SET title = 'oops'", [])
            .unwrap();
        repair_legacy_tag_text(&conn).unwrap();
        let title: String = conn
            .query_row("SELECT title FROM tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(title, "oops");
    }
}
