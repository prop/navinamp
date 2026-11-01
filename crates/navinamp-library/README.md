# navinamp-library

NaVinaMP's music library: an **incremental scanner** + **SQLite/FTS5 queries**
in one dependency-free-to-embed crate (no Tauri, no audio playback — the
Tauri shell in `src-tauri/src/library.rs` is a thin wrapper).

```rust
use std::path::PathBuf;
use navinamp_library::{Library, ScanProgress};

let library = Library::open("/app-data/navinamp.db")?;

// Scan (background-friendly: progress callback, per-file error tolerance)
let report = library.scan_roots(&[PathBuf::from("/Users/me/Music")], |p| {
    if let ScanProgress::Progress { scanned, total } = p {
        eprintln!("scanning {scanned}/{total}");
    }
})?;

// Query (FTS5-ranked, prefix-friendly, case/diacritic-insensitive)
for track in library.search("bjö", 25)? { /* ... */ }
for album in library.albums(None)? { /* ... */ }
let stats = library.stats()?; // tracks / albums / artists / total_secs / last_scan
```

## Schema

One table holds every track, local or (later, M5) remote — `source`
discriminates — so search, smart views and playlists will work identically
over both (plan §5). Albums/artists are **not** separate tables: they are
`GROUP BY album_key` views, which keeps the scanner append-only and makes
upserts trivial.

```sql
CREATE TABLE tracks (
    id              INTEGER PRIMARY KEY,
    path            TEXT UNIQUE NOT NULL,   -- canonical, lossy-UTF8 absolute path
    mtime           INTEGER NOT NULL,       -- unix MILLIS (see “Incremental”)
    size            INTEGER NOT NULL,
    title, artist, album_artist, album      -- TEXT NOT NULL DEFAULT '' (no NULL tag soup)
    album_key       TEXT NOT NULL,          -- see “Album grouping”
    genre           TEXT NOT NULL DEFAULT '',
    year, track_no, disc_no                 -- INTEGER NULL
    duration_secs   REAL NOT NULL DEFAULT 0.0,
    bitrate_kbps, samplerate, channels      -- INTEGER NULL
    codec           TEXT NOT NULL DEFAULT '',  -- 'mp3','flac','ogg','opus','m4a','wav',…
    rating          INTEGER NOT NULL DEFAULT 0,
    play_count      INTEGER NOT NULL DEFAULT 0,
    last_played_at  INTEGER,                -- unix seconds (user data)
    date_added      INTEGER NOT NULL,       -- unix seconds, set on FIRST insert
    source          TEXT NOT NULL DEFAULT 'local',
    remote_id       TEXT,                   -- Navidrome id when source='navidrome'
    art_id          TEXT                    -- local: art-cache content hash; remote: coverArt id
);
-- indexes: album_key, artist, genre, (artist, album)
```

Plus:

- **`meta(key, value)`** — a two-column bookkeeping table (`last_scan_at`,
  `art_backfilled`). It holds no track data; `tracks` remains the single
  track table.
- **`tracks_fts`** — the FTS5 index (below).
- **`<db dir>/art-cache/`** — the album-art cache (below).

Text tags are stored as `''`, never `NULL`, so display code never fights
`Option<String>`; numbers stay `NULL` when genuinely unknown. Paths are
stored via `to_string_lossy` (non-UTF-8 paths degrade to replacement chars —
an accepted trade-off for a TEXT-keyed table on macOS/Linux/Windows).

## Full-text search (FTS5)

```sql
CREATE VIRTUAL TABLE tracks_fts USING fts5(
    title, artist, album, genre,
    content='tracks', content_rowid='id'   -- external content
);
```

**Choice: external-content, trigger-synced** (over contentless): the indexed
text lives exactly once — in `tracks` — and three triggers (`AFTER INSERT /
UPDATE / DELETE`) keep the token index in lockstep with *every* write path.
There is no code path that can forget the index, and readers of `tracks`
never see a second copy drift. Cost: `bm25()`-ranked queries join back to
`tracks` by rowid (cheap) and every UPDATE re-indexes the four columns even
when only e.g. `rating` changed (irrelevant at scan cadence).

Search input is sanitized into an `AND` of quoted **prefix phrases**:
`Beatles "let it b"` → `"Beatles"* "let it b"*`. Quoting makes FTS5
operators (`AND`, `-`, `:`, …) inert and doubled embedded quotes escape
cleanly — arbitrary user input can never produce malformed/injected MATCH
syntax. The default `unicode61` tokenizer is case-insensitive and folds
Latin diacritics, so `bjö`, `BJÖ` and `bjo` all match `Björk`. Ranking:
`ORDER BY bm25(tracks_fts)` (smaller = better) with artist/album/track
tie-breakers for stable output.

## Scanner semantics

`scan_roots(&self, roots, progress) -> ScanReport` — one call, one SQLite
transaction (a scan lands wholesale or not at all):

1. **Canonicalize roots** (`fs::canonicalize`); unresolvable roots become
   report errors, never aborts. Overlapping/duplicate roots are de-duped.
2. **Walk** (walkdir, sorted per directory for determinism, symlinks not
   followed). Only audio extensions are candidates
   (`mp3 flac ogg oga opus m4a aac wav aiff ape wv`, case-insensitive);
   unknown extensions are skipped **silently**.
3. **Upsert incrementally.** A file whose stored `(path, mtime, size)`
   matches is skipped (tags not re-read). Changed files are re-read with
   lofty (tags + duration/bitrate/samplerate/channels from properties,
   codec from file type) and `UPDATE`d; new files `INSERT`ed.
   `mtime` is stored in **unix milliseconds** — fine enough to catch
   same-second rewrites.
4. **Remove vanished rows**: rows under the scanned roots whose files no
   longer exist are deleted. Rows **outside** the scanned roots are never
   touched, and a file missing from the walk but still on disk (e.g. a
   permission-denied directory) is kept — the `exists()` check makes
   partial walks safe.

Per-file failures (corrupt file, unreadable tag) are collected in
`ScanReport::errors` and the scan continues; only database-level failures
propagate as `Err`. User data (`rating`, `play_count`, `last_played_at`)
and provenance (`source`, `remote_id`, `date_added`) survive
re-tagging — the upsert refreshes only scanner-owned columns (`art_id`
among them now; remote rows are never touched by the local upsert since
`navidrome://` paths cannot collide with filesystem paths).

## Album art

Extraction happens **inside the scanner's tag-read pass** — the file is
open anyway, so art costs no extra I/O on changed files:

1. **Embedded wins**: the tag's first `CoverFront` picture (falling back
   to its first picture of any type) is that file's art.
2. **Folder art fills**: files without embedded art use a sibling image —
   `folder.jpg|jpeg|png`, `cover.*`, `front.*` (case-insensitive,
   priority order) — resolved **once per directory per scan** (memoized;
   one `read_dir` + one image read per directory).
3. **Content-addressed cache**: bytes land in
   `<db dir>/art-cache/<art_id>.<ext>` where
   `art_id = hex(sha256(content))[..16]` (16 hex chars) and the extension
   is sniffed from magic bytes (png/jpg/gif/bmp/webp — only formats a
   webview renders). Identical content → identical id → the write is
   skipped if the file exists, so a cover shared by ten tracks (or ten
   directories) is stored exactly once. Failures degrade to "no art",
   never to scan errors.

**Incrementality**: unchanged files keep their `art_id` (they are not
re-opened). One exception: the *first* art-aware scan of a database
(`meta.art_backfilled` absent) re-derives art for unchanged rows, so
libraries indexed by pre-art builds gain covers without a forced full
rescan; the marker is then set forever. A file that changes re-derives
art (possibly to `NULL` — art genuinely gone after a re-tag).

**Remote rows** never touch the cache: the Navidrome sync stores the
server's `coverArt` id directly (`RemoteTrack::art_id`), and the caller
builds authed `getCoverArt` URLs from it.

**Serving** is not this crate's business: the Tauri shell serves
`<art_id>` → cached bytes over its `navinamp-art` URI scheme. GC is
deliberately absent — cache files are content-addressed with no
refcounts, removals never delete art, and orphans are harmless (see
`art.rs`).

`albums()` picks the group's art as the **first non-null `art_id` in
disc/track order**, paired with the `source` of the track that carries
it (`Album::art_source`) so the frontend knows whether an id is a local
content hash or a Navidrome `coverArt` id.

## Remote sync (Navidrome)

`sync_remote(tracks)` mirrors the scanner for `source = 'navidrome'` rows,
fed by the Subsonic `getArtists`/`getAlbum` walk in the Tauri bridge:

- one **transaction** for the whole batch (a sync lands wholesale or not at
  all);
- rows are keyed by `path = "navidrome://<remote_id>"` — a namespace that
  can never collide with a filesystem path, so the local upsert-by-path
  and the remote upsert-by-path stay independent;
- `mtime` is pinned to `0` (no filesystem stamp) and `size` comes from the
  server's song data;
- upserts refresh only sync-owned columns (tags, properties, `art_id`);
  user data and `date_added` survive exactly as with local re-tags — the
  FTS triggers fire on the same write path, so remote rows are searchable
  instantly;
- removal deletes **only** `source = 'navidrome'` rows whose `remote_id`
  is absent from the batch — local rows are never candidates;
- `RemoteSyncReport { added, updated, removed }`; the bridge folds in
  artist/album counts, per-item errors and wall-clock time.

`TrackFilter` gained a `source` discriminator so the UI can list
`local | navidrome | both` through one query path.

`ScanReport` (all camelCase over IPC): `roots, filesFound, added, updated,
unchanged, removed, errors[{path,error}], durationMs`. Progress arrives as
`ScanProgress::{Discovered{total}, Progress{scanned,total}}` — one
`Discovered` after the walk, one `Progress` per file, so the UI layer can
throttle event emission however it likes.

## Album grouping

`album_key = lower(trim(album_artist || fallback artist)) ++ '\0' ++
lower(trim(album))` — album artist wins, track artist is the fallback, so
compilations tagged `album_artist=Various` + real artists stay one album,
and `Greatest Hits` vs `greatest hits` merge (deliberate). Untagged files
group under the empty key (rendered as “Unknown Album/Artist”). `albums()`
picks deterministic representatives (`MIN(album)`, `MIN(album-artist)`,
earliest year) where sloppily-tagged members disagree.

## Concurrency & opening

`Library::open` enables **WAL** + 5 s busy timeout + `synchronous=NORMAL`,
so multiple connections on one file coexist — this is exactly how the app
keeps queries responsive while a background scan connection writes (the
scan runs in one transaction; readers see the pre-scan snapshot until it
commits). `Library` is `Send` (asserted at compile time) but not `Sync`;
wrap it in a `Mutex` for shared access.

## Out of scope / limitations

- **No filesystem watching** (`notify` lands in a later milestone);
  rescans are cheap and incremental in the meantime.
- **Art cache garbage collection is absent by design** (see “Album
  art”): content-addressed files with no refcounts; orphans accumulate
  only when files change their art or tracks are removed. A later
  maintenance sweep can diff the cache against `tracks.art_id`.
- No writes to user data yet (`rating`, `play_count`,
  `last_played_at`) — they arrive with playback integration.
- Remote (`source='navidrome'`) rows: sync is implemented (`sync_remote`,
  see above); album-art *fetching* from the server is a later milestone.

## Testing

`cargo test -p navinamp-library` — all on temp-dir databases with
**generated** fixtures (no binary assets in-repo):

- MP3: N × a hand-crafted 417-byte MPEG-1 Layer III frame (128 kbps,
  44.1 kHz, stereo silence), tagged through **lofty itself**
  (`BoundTaggedFile` → ID3v2), so fixture writes exercise the same code
  path as real files. Validity is pinned by `fixture_smoke.rs`.
- WAV: canonical 44-byte RIFF header + PCM16 stereo, exact duration by
  construction.
- Embedded art: a minimal valid 1×1 PNG / signature-valid JPEG pushed
  through lofty's picture API; folder art is the same bytes written next
  to the tracks.

Coverage: full scan + tag/property round-trip (incl. unicode), rescan
no-op, retag → UPDATE (with `date_added` survival), delete → removal,
out-of-root rows survive, FTS prefix/case/diacritic/multi-term/limit
search, album grouping fallback, stats, corrupt-file tolerance, non-audio
skipping, unresolvable root tolerance, paging filters, progress events,
camelCase IPC shape, second-connection visibility (WAL), overlapping roots,
single-file root. Album art: embedded extraction + cache layout, folder
fallback (case-insensitive), content dedup across files and directories,
`albums()` art id + source, incremental keep/update-to-NULL semantics,
one-time backfill, `art_for_track` lookup, in-memory libraries (art
disabled), plus unit tests for magic-byte sniffing and id shapes. Remote
sync: insert with derived paths, resync → UPDATE (user data +
`date_added` survival, `art_id` refresh), remove-unseen scoped away from
local rows, empty-batch wipe, FTS visibility of remote rows, empty-id
tolerance.
