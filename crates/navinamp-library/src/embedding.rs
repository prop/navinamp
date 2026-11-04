//! Semantic-search embeddings: the per-track metadata text, its content hash,
//! and the SQLite storage/search primitives.
//!
//! Vectors live in `track_embeddings` as little-endian f32 BLOBs keyed by
//! `(track_id, model)`. Only text hashes are stored: when a tag changes, the
//! next incremental index pass notices the hash mismatch instead of trusting
//! an `mtime` (remote syncs rewrite rows without touching mtimes).
//!
//! The actual embedding call (provider `/embeddings`) lives in the Tauri layer
//! so this crate stays free of network/app concerns.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::params;
use serde::Serialize;

use crate::model::Track;
use crate::query::{track_columns, track_from_row};
use crate::{Library, Result};

/// Bump when the embedded text template changes; hashes from older templates
/// then fail the incremental check and the rows are re-embedded.
const TEXT_VERSION: &str = "v1";
/// Hard ceiling for one `embedding_jobs` request (a full library is ~21k).
const MAX_JOBS: u32 = 100_000;

/// How much of the library is currently embedded for one model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingStatus {
    /// Rows in `tracks`.
    pub total: u64,
    /// Vectors stored for the requested model.
    pub indexed: u64,
    /// Tracks with no vector or a text hash that no longer matches.
    pub stale: u64,
}

/// One track that needs (re-)embedding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingJob {
    pub track_id: i64,
    /// Compact metadata string sent to the provider.
    pub text: String,
    /// FNV-1a of `text`; stored with the vector for change detection.
    pub hash: i64,
}

/// The metadata string embedded for a track. Title/artist/album/genre/year in
/// that order, skipping empty tags. Sparse rows fall back to the file stem so
/// every track still has some identity.
fn text_for(
    title: &str,
    artist: &str,
    album: &str,
    genre: &str,
    year: Option<i64>,
    path: &str,
) -> String {
    let mut parts: Vec<String> = [title, artist, album, genre]
        .into_iter()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect();
    if let Some(year) = year {
        parts.push(year.to_string());
    }
    if parts.is_empty()
        && let Some(stem) = std::path::Path::new(path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::trim)
            .filter(|stem| !stem.is_empty())
    {
        parts.push(stem.to_owned());
    }
    format!("{TEXT_VERSION} {}", parts.join(" | "))
}

/// Deterministic FNV-1a 64-bit hash (stored as i64; sign does not matter).
fn text_hash(text: &str) -> i64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash as i64
}

fn vector_to_blob(vector: &[f32]) -> Vec<u8> {
    let mut blob = Vec::with_capacity(vector.len() * 4);
    for value in vector {
        blob.extend_from_slice(&value.to_le_bytes());
    }
    blob
}

fn blob_to_vector(blob: &[u8]) -> Option<Vec<f32>> {
    let (chunks, remainder) = blob.as_chunks::<4>();
    if !remainder.is_empty() {
        return None;
    }
    Some(
        chunks
            .iter()
            .map(|chunk| f32::from_le_bytes(*chunk))
            .collect(),
    )
}

impl Library {
    /// Current index coverage for `model`.
    pub fn embedding_status(&self, model: &str) -> Result<EmbeddingStatus> {
        let total = self
            .conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |row| {
                row.get::<_, i64>(0)
            })? as u64;
        let indexed = self.conn.query_row(
            "SELECT COUNT(*)
               FROM track_embeddings e
               JOIN tracks t ON t.id = e.track_id
              WHERE e.model = ?1",
            [model],
            |row| row.get::<_, i64>(0),
        )? as u64;

        let mut indexed_hashes: HashMap<i64, i64> = HashMap::new();
        let mut stmt = self
            .conn
            .prepare("SELECT track_id, text_hash FROM track_embeddings WHERE model = ?1")?;
        for row in stmt.query_map([model], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })? {
            let (id, hash) = row?;
            indexed_hashes.insert(id, hash);
        }

        let mut stale = 0u64;
        let mut stmt = self.conn.prepare(
            "SELECT id, title, artist, album, genre, year, path
               FROM tracks",
        )?;
        for row in stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, String>(6)?,
            ))
        })? {
            let (id, title, artist, album, genre, year, path) = row?;
            let hash = text_hash(&text_for(&title, &artist, &album, &genre, year, &path));
            if indexed_hashes.get(&id) != Some(&hash) {
                stale += 1;
            }
        }
        Ok(EmbeddingStatus {
            total,
            indexed,
            stale,
        })
    }

    /// Tracks that need embedding for `model`, in library order. `limit`
    /// bounds one indexing pass; `None` means the whole stale set.
    pub fn embedding_jobs(&self, model: &str, limit: Option<u32>) -> Result<Vec<EmbeddingJob>> {
        let limit = limit.unwrap_or(MAX_JOBS).min(MAX_JOBS) as usize;
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut indexed_hashes: HashMap<i64, i64> = HashMap::new();
        let mut stmt = self
            .conn
            .prepare("SELECT track_id, text_hash FROM track_embeddings WHERE model = ?1")?;
        for row in stmt.query_map([model], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })? {
            let (id, hash) = row?;
            indexed_hashes.insert(id, hash);
        }

        let mut jobs = Vec::new();
        let mut stmt = self.conn.prepare(
            "SELECT id, title, artist, album, genre, year, path
               FROM tracks
              ORDER BY id",
        )?;
        for row in stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, String>(6)?,
            ))
        })? {
            let (id, title, artist, album, genre, year, path) = row?;
            let text = text_for(&title, &artist, &album, &genre, year, &path);
            let hash = text_hash(&text);
            if indexed_hashes.get(&id) != Some(&hash) {
                jobs.push(EmbeddingJob {
                    track_id: id,
                    text,
                    hash,
                });
                if jobs.len() >= limit {
                    break;
                }
            }
        }
        Ok(jobs)
    }

    /// Drop every vector for `model` (a full rebuild).
    pub fn embedding_clear(&self, model: &str) -> Result<u64> {
        Ok(self
            .conn
            .execute("DELETE FROM track_embeddings WHERE model = ?1", [model])? as u64)
    }

    /// Upsert a batch of vectors. `(track_id, text_hash, vector)`.
    pub fn embedding_upsert(&self, model: &str, rows: &[(i64, i64, Vec<f32>)]) -> Result<usize> {
        if rows.is_empty() {
            return Ok(0);
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let tx = self.conn.unchecked_transaction()?;
        let mut written = 0usize;
        {
            let mut stmt = tx.prepare(
                // The EXISTS guard tolerates a scan deleting the track while
                // an index batch is in flight (the FK would otherwise abort
                // the whole transaction).
                "INSERT INTO track_embeddings
                     (track_id, model, dims, text_hash, vector, updated_at)
                 SELECT ?1, ?2, ?3, ?4, ?5, ?6
                  WHERE EXISTS (SELECT 1 FROM tracks WHERE id = ?1)
                 ON CONFLICT(track_id, model) DO UPDATE SET
                     dims = excluded.dims,
                     text_hash = excluded.text_hash,
                     vector = excluded.vector,
                     updated_at = excluded.updated_at",
            )?;
            for (track_id, hash, vector) in rows {
                if vector.is_empty() {
                    continue;
                }
                let blob = vector_to_blob(vector);
                written += stmt.execute(params![
                    track_id,
                    model,
                    vector.len() as i64,
                    hash,
                    blob,
                    now
                ])?;
            }
        }
        tx.commit()?;
        Ok(written)
    }

    /// Every stored vector for `model`, in track-id order. Malformed BLOBs
    /// (only possible after a partial write) are skipped.
    pub fn embedding_vectors(&self, model: &str) -> Result<Vec<(i64, Vec<f32>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.track_id, e.vector
               FROM track_embeddings e
               JOIN tracks t ON t.id = e.track_id
              WHERE e.model = ?1
              ORDER BY e.track_id",
        )?;
        let mut out = Vec::new();
        for row in stmt.query_map([model], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })? {
            let (id, blob) = row?;
            if let Some(vector) = blob_to_vector(&blob)
                && !vector.is_empty()
            {
                out.push((id, vector));
            }
        }
        Ok(out)
    }

    /// Fetch tracks by id, preserving the order of `ids`.
    pub fn tracks_by_ids(&self, ids: &[i64]) -> Result<Vec<Track>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT {} FROM tracks WHERE id IN ({placeholders})",
            track_columns("")
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut found: HashMap<i64, Track> = HashMap::with_capacity(ids.len());
        let mut rows = stmt.query(rusqlite::params_from_iter(ids.iter()))?;
        while let Some(row) = rows.next()? {
            let track = track_from_row(row)?;
            found.insert(track.id, track);
        }
        Ok(ids.iter().filter_map(|id| found.remove(id)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library_with_tracks() -> Library {
        let library = Library::open(":memory:").expect("open");
        library
            .conn
            .execute_batch(
                "INSERT INTO tracks
                     (path, mtime, size, title, artist, album, album_key,
                      genre, year, duration_secs, date_added)
                 VALUES
                     ('/music/a.flac', 1, 10, 'First song', 'Artist A',
                      'Album A', 'artist a/album a', 'Jazz', 1972, 120.0, 1),
                     ('/music/b.flac', 1, 10, 'Second song', 'Artist B',
                      'Album B', 'artist b/album b', 'Rock', 1999, 90.0, 1);",
            )
            .expect("insert tracks");
        library
    }

    #[test]
    fn text_includes_tags_and_version() {
        let text = text_for("Song", "Artist", "Album", "Jazz", Some(1970), "/x");
        assert!(text.starts_with("v1 "));
        assert!(text.contains("Song"));
        assert!(text.contains("Artist"));
        assert!(text.contains("Album"));
        assert!(text.contains("Jazz"));
        assert!(text.contains("1970"));
    }

    #[test]
    fn sparse_text_falls_back_to_file_stem() {
        let text = text_for("", " ", "", "", None, "/music/Untitled Track.flac");
        assert_eq!(text, "v1 Untitled Track");
    }

    #[test]
    fn status_jobs_and_upsert_round_trip() {
        let library = library_with_tracks();
        let status = library.embedding_status("test-model").expect("status");
        assert_eq!(status.total, 2);
        assert_eq!(status.indexed, 0);
        assert_eq!(status.stale, 2);

        let jobs = library.embedding_jobs("test-model", None).expect("jobs");
        assert_eq!(jobs.len(), 2);
        assert!(jobs[0].text.contains("First song"));

        let rows: Vec<(i64, i64, Vec<f32>)> = jobs
            .iter()
            .map(|job| (job.track_id, job.hash, vec![0.5, 0.25, 0.125]))
            .collect();
        assert_eq!(
            library
                .embedding_upsert("test-model", &rows)
                .expect("upsert"),
            2
        );

        let status = library.embedding_status("test-model").expect("status");
        assert_eq!(status.indexed, 2);
        assert_eq!(status.stale, 0);
        assert!(
            library
                .embedding_jobs("test-model", None)
                .expect("jobs")
                .is_empty()
        );

        let vectors = library.embedding_vectors("test-model").expect("vectors");
        assert_eq!(vectors.len(), 2);
        assert_eq!(vectors[0].1, vec![0.5, 0.25, 0.125]);

        let ids: Vec<i64> = jobs.iter().map(|job| job.track_id).collect();
        let tracks = library.tracks_by_ids(&ids).expect("tracks");
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].id, ids[0]);
        assert_eq!(tracks[1].id, ids[1]);
    }

    #[test]
    fn changed_metadata_makes_the_vector_stale() {
        let library = library_with_tracks();
        let jobs = library.embedding_jobs("test-model", None).expect("jobs");
        let rows: Vec<(i64, i64, Vec<f32>)> = jobs
            .iter()
            .map(|job| (job.track_id, job.hash, vec![1.0]))
            .collect();
        library
            .embedding_upsert("test-model", &rows)
            .expect("upsert");
        library
            .conn
            .execute(
                "UPDATE tracks SET title = 'Renamed song' WHERE id = ?1",
                [jobs[0].track_id],
            )
            .expect("rename");
        let status = library.embedding_status("test-model").expect("status");
        assert_eq!(status.indexed, 2);
        assert_eq!(status.stale, 1);
        let pending = library.embedding_jobs("test-model", None).expect("jobs");
        assert_eq!(pending.len(), 1);
        assert!(pending[0].text.contains("Renamed song"));
    }

    #[test]
    fn clearing_one_model_keeps_other_models() {
        let library = library_with_tracks();
        let jobs = library.embedding_jobs("a", None).expect("jobs");
        let rows: Vec<(i64, i64, Vec<f32>)> = jobs
            .iter()
            .map(|job| (job.track_id, job.hash, vec![1.0]))
            .collect();
        library.embedding_upsert("a", &rows).expect("a");
        library.embedding_upsert("b", &rows).expect("b");
        assert_eq!(library.embedding_clear("a").expect("clear"), 2);
        assert!(library.embedding_vectors("a").expect("a").is_empty());
        assert_eq!(library.embedding_vectors("b").expect("b").len(), 2);
    }
}
