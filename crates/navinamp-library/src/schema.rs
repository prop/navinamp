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
