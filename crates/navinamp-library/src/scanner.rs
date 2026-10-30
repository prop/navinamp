//! Incremental scanner: walk roots, read tags with lofty, upsert into
//! `tracks`, remove vanished rows under the scanned roots. Album art is
//! derived in the same pass (see `art.rs` for the extraction policy and
//! cache layout).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension};
use walkdir::WalkDir;

use lofty::file::{AudioFile, FileType, TaggedFileExt};
use lofty::tag::{Accessor, ItemKey};

use crate::art;
use crate::model::{ScanFileError, ScanProgress, ScanReport};

/// Audio extensions the scanner picks up (compared case-insensitively).
/// Unknown extensions are skipped silently — a music folder is full of
/// covers, logs and playlists that are none of the scanner's business.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "ogg", "oga", "opus", "m4a", "aac", "wav", "aiff", "ape", "wv",
];

/// Everything lofty told us about one file, schema-shaped.
struct FreshTrack {
    mtime: i64,
    size: i64,
    title: String,
    artist: String,
    album_artist: String,
    album: String,
    album_key: String,
    genre: String,
    year: Option<i64>,
    track_no: Option<i64>,
    disc_no: Option<i64>,
    duration_secs: f64,
    bitrate_kbps: Option<i64>,
    samplerate: Option<i64>,
    channels: Option<i64>,
    codec: String,
    /// The file's chosen embedded picture (front cover preferred), when it
    /// carries one. Cached + hashed by the scan loop (`art::store`).
    embedded_art: Option<Vec<u8>>,
}

/// Normalized album grouping key: the album artist (falling back to the
/// track artist), trimmed and lower-cased, a NUL separator, then the album
/// title trimmed and lower-cased. Lower-casing deliberately merges
/// `Greatest Hits` / `greatest hits` split across sloppy tags; it never
/// merges different artists or albums.
pub fn album_key(album_artist: &str, artist: &str, album: &str) -> String {
    let grouping_artist = if album_artist.trim().is_empty() {
        artist.trim()
    } else {
        album_artist.trim()
    };
    let mut key = String::with_capacity(grouping_artist.len() + album.len() + 1);
    key.push_str(&grouping_artist.to_lowercase());
    key.push('\0');
    key.push_str(&album.trim().to_lowercase());
    key
}

/// Unix milliseconds since the epoch for `t` (0 for pre-epoch clocks).
fn unix_ms(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// True if `path`'s extension is one of [`AUDIO_EXTENSIONS`]. Also used by
/// `crate::expand_paths` (recursive directory adds).
pub(crate) fn has_audio_extension(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => AUDIO_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()),
        None => false,
    }
}

/// Short codec id from lofty's detected file type.
fn codec_name(file_type: FileType) -> String {
    // Custom resolvers bring their own name; use it as-is.
    if let FileType::Custom(name) = file_type {
        return name.to_owned();
    }
    let name = match file_type {
        FileType::Mpeg => "mp3",
        FileType::Flac => "flac",
        FileType::Vorbis => "ogg",
        FileType::Opus => "opus",
        FileType::Mp4 => "m4a",
        FileType::Aac => "aac",
        FileType::Wav => "wav",
        FileType::Aiff => "aiff",
        FileType::Ape => "ape",
        FileType::WavPack => "wv",
        FileType::Mpc => "mpc",
        FileType::Speex => "spx",
        // `FileType` is `#[non_exhaustive]`; future types keep their Debug
        // name, lower-cased.
        _ => return format!("{file_type:?}").to_lowercase(),
    };
    name.to_owned()
}

/// Stat + tag-read one file. All I/O and lofty errors surface as `Err(String)`
/// so the scan loop can record them per-file and march on.
fn read_track(path: &Path) -> Result<FreshTrack, String> {
    let meta = fs::metadata(path).map_err(|e| format!("stat: {e}"))?;
    let tagged = lofty::read_from_path(path).map_err(|e| format!("parse: {e}"))?;
    let props = tagged.properties();
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());

    // Prefer the pure audio bitrate; some containers only report overall.
    let bitrate = props
        .audio_bitrate()
        .or(props.overall_bitrate())
        .map(i64::from);

    let (title, artist, album_artist, album, genre, year, track_no, disc_no) = match tag {
        Some(t) => (
            // `Accessor` getters hand back `Option<Cow<str>>`; `Cow<str>`
            // defaults to `""`, so this is a clean String with no clones.
            t.title().unwrap_or_default().into_owned(),
            t.artist().unwrap_or_default().into_owned(),
            t.get_string(ItemKey::AlbumArtist)
                .unwrap_or_default()
                .to_owned(),
            t.album().unwrap_or_default().into_owned(),
            t.genre().unwrap_or_default().into_owned(),
            t.date().filter(|d| d.year > 0).map(|d| i64::from(d.year)),
            t.track().map(i64::from),
            t.disk().map(i64::from),
        ),
        None => (
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            None,
            None,
            None,
        ),
    };
    let album_key = album_key(&album_artist, &artist, &album);

    Ok(FreshTrack {
        mtime: unix_ms(meta.modified().unwrap_or(UNIX_EPOCH)),
        size: i64::try_from(meta.len()).unwrap_or(i64::MAX),
        title,
        artist,
        album_artist,
        album,
        album_key,
        genre,
        year,
        track_no,
        disc_no,
        duration_secs: props.duration().as_secs_f64(),
        bitrate_kbps: bitrate,
        samplerate: props.sample_rate().map(i64::from),
        channels: props.channels().map(i64::from),
        codec: codec_name(tagged.file_type()),
        embedded_art: embedded_art(&tagged),
    })
}

/// The file's chosen embedded picture bytes — front cover preferred, first
/// picture as fallback (see `art::embedded_picture`). Shared by the
/// regular tag-read pass and the backfill pass.
fn embedded_art(tagged: &lofty::file::TaggedFile) -> Option<Vec<u8>> {
    let pictures = tagged
        .primary_tag()
        .or_else(|| tagged.first_tag())
        .map(|tag| tag.pictures())
        .unwrap_or(&[]);
    art::embedded_picture(pictures).map(|picture| picture.data().to_vec())
}

/// Derives and caches the art id for one file: embedded bytes when present
/// (and cacheable), else the directory's folder image (memoized per scan).
fn store_art(
    embedded: Option<&[u8]>,
    path: &Path,
    art_dir: Option<&Path>,
    folder_memo: &mut HashMap<PathBuf, Option<String>>,
) -> Option<String> {
    embedded
        .and_then(|bytes| art::store(art_dir, bytes))
        .or_else(|| {
            let dir = path.parent()?;
            art::folder_art(dir, art_dir, folder_memo)
        })
}

/// Art-only read for the backfill pass (unchanged files of a pre-art
/// database): parse the file once for its picture, folder fallback as
/// above. Any I/O or parse failure is `None` — backfill must never turn a
/// healthy row into an error.
fn read_art(
    path: &Path,
    art_dir: Option<&Path>,
    folder_memo: &mut HashMap<PathBuf, Option<String>>,
) -> Option<String> {
    let tagged = lofty::read_from_path(path).ok()?;
    store_art(embedded_art(&tagged).as_deref(), path, art_dir, folder_memo)
}

/// Runs the full incremental scan (see `README.md` for semantics).
///
/// `art_dir` is where album-art bytes are cached (`None` disables art
/// extraction — in-memory databases). Progress is called once after
/// discovery and once per processed file, so the UI layer can throttle
/// event emission however it likes.
pub(super) fn scan_roots(
    conn: &Connection,
    art_dir: Option<&Path>,
    roots: &[PathBuf],
    mut progress: impl FnMut(ScanProgress),
) -> rusqlite::Result<ScanReport> {
    let started = SystemTime::now();
    let mut report = ScanReport::default();
    let mut errors: Vec<ScanFileError> = Vec::new();

    // Canonicalize the roots (resolves symlinks like `~/Music` on macOS)
    // before anything else: every stored path is then canonical too, so
    // prefix matching for removals is exact.
    let mut canon: Vec<PathBuf> = Vec::with_capacity(roots.len());
    for root in roots {
        match fs::canonicalize(root) {
            Ok(c) => {
                if !canon.contains(&c) {
                    canon.push(c);
                }
            }
            Err(e) => errors.push(ScanFileError {
                path: root.display().to_string(),
                error: format!("cannot resolve root: {e}"),
            }),
        }
    }
    report.roots = canon.iter().map(|r| r.display().to_string()).collect();

    // Phase 1 — discover candidate audio files (de-duped across overlapping
    // roots via `seen`), sorted per directory for deterministic scans.
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for root in &canon {
        for entry in WalkDir::new(root).sort_by_file_name() {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    errors.push(ScanFileError {
                        path: e
                            .path()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| root.display().to_string()),
                        error: format!("walk: {e}"),
                    });
                    continue;
                }
            };
            if !entry.file_type().is_file() || !has_audio_extension(entry.path()) {
                continue;
            }
            if seen.insert(entry.into_path()) {
                report.files_found += 1;
            }
        }
    }
    let total = u64::try_from(seen.len()).unwrap_or(u64::MAX);
    progress(ScanProgress::Discovered { total });

    // Phase 2 + 3 in one transaction: a scan either lands wholesale or not
    // at all. `unchecked_transaction` because `Library` hands out `&self`.
    let tx = conn.unchecked_transaction()?;
    {
        let mut lookup = tx.prepare("SELECT mtime, size, art_id FROM tracks WHERE path = ?1")?;
        let mut upsert = tx.prepare(UPSERT_SQL)?;
        let mut set_art = tx.prepare("UPDATE tracks SET art_id = ?1 WHERE path = ?2")?;
        let mut delete = tx.prepare("DELETE FROM tracks WHERE id = ?1")?;

        // First art-aware scan of this database? Rows indexed by pre-art
        // builds carry no art and their files are "unchanged", so they
        // would never gain covers — backfill once, then never again.
        let backfill_art = tx
            .query_row(
                "SELECT value FROM meta WHERE key = 'art_backfilled'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .is_none();

        // Folder-image resolution is memoized per directory: one read_dir
        // + one image read per directory per scan, however many tracks it
        // holds (and content-addressing dedups even across directories).
        let mut folder_memo: HashMap<PathBuf, Option<String>> = HashMap::new();

        // Phase 2 — read/upsert each candidate, skipping unchanged files.
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        for (idx, path) in seen.iter().enumerate() {
            let path_str = path.to_string_lossy().into_owned();

            // Stat; failures are per-file errors, never fatal.
            let stat = fs::metadata(path).map(|meta| {
                (
                    unix_ms(meta.modified().unwrap_or(UNIX_EPOCH)),
                    i64::try_from(meta.len()).unwrap_or(i64::MAX),
                )
            });
            let stat = match stat {
                Ok(stat) => stat,
                Err(e) => {
                    errors.push(ScanFileError {
                        path: path_str.clone(),
                        error: format!("stat: {e}"),
                    });
                    progress(ScanProgress::Progress {
                        scanned: u64::try_from(idx + 1).unwrap_or(u64::MAX),
                        total,
                    });
                    continue;
                }
            };

            // One lookup decides both "changed?" and "new?" up front.
            let existing = lookup
                .query_row([&path_str], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                })
                .optional()?;
            let was_new = existing.is_none();
            let unchanged = existing
                .as_ref()
                .is_some_and(|(m, s, _)| *m == stat.0 && *s == stat.1);

            if unchanged {
                // Backfill pass only: unchanged files are never re-read
                // otherwise — they keep whatever art_id they already have.
                if backfill_art {
                    let existing_art = existing.and_then(|(_, _, art)| art);
                    let art_id = read_art(path, art_dir, &mut folder_memo);
                    if art_id != existing_art {
                        set_art.execute(rusqlite::params![art_id, path_str])?;
                    }
                }
                report.unchanged += 1;
            } else {
                match read_track(path) {
                    Ok(t) => {
                        let art_id =
                            store_art(t.embedded_art.as_deref(), path, art_dir, &mut folder_memo);
                        upsert.execute(rusqlite::params![
                            path_str,
                            t.mtime,
                            t.size,
                            t.title,
                            t.artist,
                            t.album_artist,
                            t.album,
                            t.album_key,
                            t.genre,
                            t.year,
                            t.track_no,
                            t.disc_no,
                            t.duration_secs,
                            t.bitrate_kbps,
                            t.samplerate,
                            t.channels,
                            t.codec,
                            now_secs,
                            art_id,
                        ])?;
                        if was_new {
                            report.added += 1;
                        } else {
                            report.updated += 1;
                        }
                    }
                    Err(e) => errors.push(ScanFileError {
                        path: path_str,
                        error: e,
                    }),
                }
            }

            progress(ScanProgress::Progress {
                scanned: u64::try_from(idx + 1).unwrap_or(u64::MAX),
                total,
            });
        }

        // Phase 3 — remove rows under the scanned roots whose files are gone.
        // Rows outside the roots are never touched, and a missing-from-`seen`
        // file that still exists on disk (a walk error, e.g. a
        // permission-denied directory) is kept, hence the `exists()` check.
        {
            let mut stmt = tx.prepare("SELECT id, path FROM tracks WHERE source = 'local'")?;
            let candidates: Vec<(i64, String)> = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<_, _>>()?;
            for (id, row_path) in candidates {
                let path = PathBuf::from(&row_path);
                if !canon.iter().any(|r| path.starts_with(r)) || seen.contains(&path) {
                    continue;
                }
                if !path.exists() {
                    delete.execute([id])?;
                    report.removed += 1;
                }
            }
        }

        // The art backfill (if any) ran to completion inside this
        // transaction; mark it done so it never runs again.
        if backfill_art {
            tx.prepare(
                "INSERT INTO meta(key, value) VALUES ('art_backfilled', '1')
                        ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            )?
            .execute([])?;
        }
    }

    report.duration_ms = started.elapsed().map(|d| d.as_millis() as u64).unwrap_or(0);
    tx.prepare(
        "INSERT INTO meta(key, value) VALUES ('last_scan_at', ?1)
                ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )?
    .execute([unix_ms(SystemTime::now()).to_string()])?;
    tx.commit()?;

    report.errors = errors;
    Ok(report)
}

/// Insert-or-retag. On conflict, everything the scanner owns is refreshed
/// — including `art_id`, derived in the same pass (embedded picture,
/// folder-image fallback; see `art.rs`); user data (`rating`,
/// `play_count`, `last_played_at`) and provenance (`source`, `remote_id`,
/// `date_added`) survive untouched. Remote rows are never touched here:
/// their paths (`navidrome://…`) cannot collide with filesystem paths.
const UPSERT_SQL: &str = "\
INSERT INTO tracks (
    path, mtime, size, title, artist, album_artist, album, album_key, genre,
    year, track_no, disc_no, duration_secs, bitrate_kbps, samplerate,
    channels, codec, date_added, art_id
) VALUES (
    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19
)
ON CONFLICT(path) DO UPDATE SET
    mtime         = excluded.mtime,
    size          = excluded.size,
    title         = excluded.title,
    artist        = excluded.artist,
    album_artist  = excluded.album_artist,
    album         = excluded.album,
    album_key     = excluded.album_key,
    genre         = excluded.genre,
    year          = excluded.year,
    track_no      = excluded.track_no,
    disc_no       = excluded.disc_no,
    duration_secs = excluded.duration_secs,
    bitrate_kbps  = excluded.bitrate_kbps,
    samplerate    = excluded.samplerate,
    channels      = excluded.channels,
    codec         = excluded.codec,
    art_id        = excluded.art_id";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn album_key_prefers_album_artist_and_normalizes() {
        let with_both = album_key("The Beatles", "John Lennon", "Let It Be");
        let artist_only = album_key("", "The Beatles", "let it be");
        let whitespace = album_key(" The BEATLES ", "", " Let It Be ");
        assert_eq!(with_both, artist_only);
        assert_eq!(with_both, whitespace);
        assert!(with_both.contains('\0'));
        // different album -> different key
        assert_ne!(with_both, album_key("The Beatles", "", "Abbey Road"));
    }

    #[test]
    fn extension_filter_matches_case_insensitively() {
        assert!(has_audio_extension(Path::new("/m/Track.MP3")));
        assert!(has_audio_extension(Path::new("/m/a.ogg")));
        assert!(has_audio_extension(Path::new("/m/a.Opus")));
        assert!(!has_audio_extension(Path::new("/m/cover.png")));
        assert!(!has_audio_extension(Path::new("/m/track"))); // no extension
    }
}
