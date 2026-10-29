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

/// Applies the full schema, idempotently.
pub(super) fn init(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)
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
    play_count      INTEGER NOT NULL DEFAULT 0,
    last_played_at  INTEGER,                   -- unix seconds
    date_added      INTEGER NOT NULL,          -- unix seconds
    source          TEXT NOT NULL DEFAULT 'local',
    remote_id       TEXT,
    art_id          TEXT
);

CREATE INDEX IF NOT EXISTS idx_tracks_album_key ON tracks(album_key);
CREATE INDEX IF NOT EXISTS idx_tracks_artist    ON tracks(artist);
CREATE INDEX IF NOT EXISTS idx_tracks_genre     ON tracks(genre);
CREATE INDEX IF NOT EXISTS idx_tracks_artist_album ON tracks(artist, album);

-- Full-text index for search; text lives in `tracks` (external content) and
-- the triggers below keep the token index in lockstep with every write path.
CREATE VIRTUAL TABLE IF NOT EXISTS tracks_fts USING fts5(
    title, artist, album, genre,
    content='tracks', content_rowid='id'
);

CREATE TRIGGER IF NOT EXISTS tracks_fts_ins AFTER INSERT ON tracks BEGIN
    INSERT INTO tracks_fts(rowid, title, artist, album, genre)
    VALUES (new.id, new.title, new.artist, new.album, new.genre);
END;

CREATE TRIGGER IF NOT EXISTS tracks_fts_del AFTER DELETE ON tracks BEGIN
    INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album, genre)
    VALUES ('delete', old.id, old.title, old.artist, old.album, old.genre);
END;

CREATE TRIGGER IF NOT EXISTS tracks_fts_upd AFTER UPDATE ON tracks BEGIN
    INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album, genre)
    VALUES ('delete', old.id, old.title, old.artist, old.album, old.genre);
    INSERT INTO tracks_fts(rowid, title, artist, album, genre)
    VALUES (new.id, new.title, new.artist, new.album, new.genre);
END;

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
COMMIT;
";
