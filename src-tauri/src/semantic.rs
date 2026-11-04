//! Semantic search: embeddings from the configured OpenAI-compatible
//! provider, a local SQLite vector index, and cosine top-k retrieval.
//!
//! The provider only ever sees the compact metadata string built by
//! `navinamp_library` (title/artist/album/genre/year) — never file paths,
//! credentials or the library itself. Indexing is explicit and incremental:
//! only tracks with no vector or a changed text hash are sent.
//!
//! Vectors are stored per embedding model, so switching providers does not
//! destroy the old index; switching back reuses it. A prompt embedding is one
//! `/embeddings` call, then brute-force cosine over the local BLOBs (tens of
//! milliseconds at library scale).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use navinamp_library::{Library, Track};
use navinamp_llm::ProviderKind;
use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, State};

use crate::ai::{self, AiConfig};
use crate::library::{self, LibraryState, with_library};

/// Progress event emitted once per embedded batch.
const PROGRESS_EVENT: &str = "semantic-progress";
/// Metadata strings per provider request (small enough for local models).
const BATCH: usize = 32;
const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 500;

/// One index build at a time. The guard resets the flag even on an error or
/// a dropped command future.
#[derive(Default)]
pub struct SemanticState {
    indexing: Arc<AtomicBool>,
}

struct IndexGuard(Arc<AtomicBool>);

impl Drop for IndexGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticStatus {
    /// Model + provider are both ready to embed/query.
    pub configured: bool,
    /// Provider/key/endpoint are ready (the model may still be missing).
    pub provider_ready: bool,
    pub model: String,
    /// Tracks in the library.
    pub total: u64,
    /// Vectors stored for `model`.
    pub indexed: u64,
    /// Tracks missing a vector or whose metadata changed.
    pub stale: u64,
    /// Actionable setup hint when `configured` is false.
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticIndexReport {
    pub model: String,
    pub embedded: u64,
    /// Number of tracks that needed embedding when the pass started.
    pub total: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticSearchReport {
    pub tracks: Vec<Track>,
    pub model: String,
    /// Indexed vectors usable with this query's dimension.
    pub indexed: u64,
    pub dims: usize,
    pub explanation: String,
}

fn status_message(config: &AiConfig) -> Option<String> {
    if config.provider == ProviderKind::Anthropic {
        return Some(
            "Anthropic has no embeddings endpoint — use an OpenAI-compatible provider for SEMANTIC search."
                .into(),
        );
    }
    if config.embedding_model.trim().is_empty() {
        return Some(
            "Set an embedding model in AI settings (e.g. text-embedding-3-small, or nomic-embed-text for Ollama)."
                .into(),
        );
    }
    if !config.enabled {
        return Some("Enable AI features in AI settings to use SEMANTIC search.".into());
    }
    if config.api_key.trim().is_empty() && !ai::is_local_endpoint(&config.base_url) {
        return Some(
            "Add an API key, or use a local OpenAI-compatible endpoint, to build the semantic index."
                .into(),
        );
    }
    None
}

fn not_configured(config: &AiConfig) -> String {
    status_message(config).unwrap_or_else(|| "semantic search is not configured".into())
}

#[tauri::command]
pub fn semantic_status(
    app: AppHandle,
    state: State<'_, LibraryState>,
) -> Result<SemanticStatus, String> {
    let config = ai::load_config(&app);
    let model = config.embedding_model.trim().to_owned();
    let (total, indexed, stale) = with_library(&app, &state, |library| {
        let total = library.track_count(None).map_err(|e| e.to_string())?;
        if model.is_empty() {
            return Ok((total, 0, 0));
        }
        let status = library
            .embedding_status(&model)
            .map_err(|e| e.to_string())?;
        Ok((status.total, status.indexed, status.stale))
    })?;
    Ok(SemanticStatus {
        configured: ai::embeddings_configured(&config),
        provider_ready: ai::embedding_provider_ready(&config),
        model,
        total,
        indexed,
        stale,
        message: status_message(&config),
    })
}

/// Embed every missing/changed track (or rebuild the whole model index).
/// Runs against a separate SQLite connection so the UI stays responsive.
#[tauri::command]
pub async fn semantic_index(
    app: AppHandle,
    state: State<'_, SemanticState>,
    rebuild: Option<bool>,
) -> Result<SemanticIndexReport, String> {
    let config = ai::load_config(&app);
    if !ai::embeddings_configured(&config) {
        return Err(not_configured(&config));
    }
    if state.indexing.swap(true, Ordering::SeqCst) {
        return Err("a semantic index build is already running".into());
    }
    let _guard = IndexGuard(Arc::clone(&state.indexing));

    let model = config.embedding_model.trim().to_owned();
    let llm = config.llm();
    let library =
        Library::open(library::db_file(&app)?).map_err(|e| format!("cannot open library: {e}"))?;
    if rebuild.unwrap_or(false) {
        library.embedding_clear(&model).map_err(|e| e.to_string())?;
    }
    let jobs = library
        .embedding_jobs(&model, None)
        .map_err(|e| e.to_string())?;
    let total = jobs.len() as u64;
    if total == 0 {
        return Ok(SemanticIndexReport {
            model,
            embedded: 0,
            total: 0,
        });
    }

    let mut embedded = 0u64;
    for chunk in jobs.chunks(BATCH) {
        let inputs: Vec<String> = chunk.iter().map(|job| job.text.clone()).collect();
        let vectors = navinamp_llm::embed(&llm, &model, &inputs)
            .await
            .map_err(|e| config.scrubbed(e))?;
        if vectors.len() != chunk.len() {
            return Err("embedding provider returned the wrong number of vectors".into());
        }
        let rows: Vec<(i64, i64, Vec<f32>)> = chunk
            .iter()
            .zip(vectors)
            .map(|(job, vector)| (job.track_id, job.hash, vector))
            .collect();
        let written = library
            .embedding_upsert(&model, &rows)
            .map_err(|e| e.to_string())?;
        embedded += written as u64;
        let _ = app.emit(
            PROGRESS_EVENT,
            json!({
                "model": &model,
                "done": embedded,
                "total": total,
            }),
        );
    }
    Ok(SemanticIndexReport {
        model,
        embedded,
        total,
    })
}

#[tauri::command]
pub async fn semantic_search(
    app: AppHandle,
    state: State<'_, LibraryState>,
    prompt: String,
    limit: Option<u32>,
) -> Result<SemanticSearchReport, String> {
    let clean = prompt.trim();
    if clean.is_empty() {
        return Err("enter a search prompt first".into());
    }
    let config = ai::load_config(&app);
    if !ai::embeddings_configured(&config) {
        return Err(not_configured(&config));
    }
    let model = config.embedding_model.trim().to_owned();
    let status = with_library(&app, &state, |library| {
        library.embedding_status(&model).map_err(|e| e.to_string())
    })?;
    if status.indexed == 0 {
        return Err("the semantic index is empty — build it with INDEX LIBRARY first".into());
    }

    let query = navinamp_llm::embed(&config.llm(), &model, &[clean.to_owned()])
        .await
        .map_err(|e| config.scrubbed(e))?
        .into_iter()
        .next()
        .ok_or_else(|| "embedding provider returned no vector".to_string())?;

    let (tracks, indexed, dims) = with_library(&app, &state, |library| {
        let vectors = library
            .embedding_vectors(&model)
            .map_err(|e| e.to_string())?;
        let (ids, usable) = cosine_top_k(&query, &vectors, limit.unwrap_or(DEFAULT_LIMIT as u32));
        let tracks = library.tracks_by_ids(&ids).map_err(|e| e.to_string())?;
        if usable == 0 {
            return Err(
                "the indexed vectors do not match this embedding model's dimension — rebuild the index"
                    .into(),
            );
        }
        Ok((tracks, usable as u64, query.len()))
    })?;

    let explanation = format!(
        "Semantic similarity · {model} · {indexed} indexed track{}",
        if indexed == 1 { "" } else { "s" }
    );
    Ok(SemanticSearchReport {
        tracks,
        model,
        indexed,
        dims,
        explanation,
    })
}

/// Cosine top-k. Vectors with a different dimension are skipped (a model
/// change leaves old rows behind); ties break by track id for determinism.
fn cosine_top_k(query: &[f32], vectors: &[(i64, Vec<f32>)], limit: u32) -> (Vec<i64>, usize) {
    let limit = (limit.max(1) as usize).min(MAX_LIMIT);
    let query_norm = query
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    if query_norm == 0.0 {
        return (Vec::new(), 0);
    }
    let mut scored: Vec<(i64, f64)> = Vec::with_capacity(vectors.len());
    let mut usable = 0usize;
    for (id, vector) in vectors {
        if vector.len() != query.len() {
            continue;
        }
        usable += 1;
        let mut dot = 0.0f64;
        let mut norm = 0.0f64;
        for (a, b) in query.iter().zip(vector) {
            let a = f64::from(*a);
            let b = f64::from(*b);
            dot += a * b;
            norm += b * b;
        }
        if norm == 0.0 {
            continue;
        }
        scored.push((*id, dot / (query_norm * norm.sqrt())));
    }
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    scored.truncate(limit);
    (scored.into_iter().map(|(id, _)| id).collect(), usable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_ranks_by_similarity_and_skips_wrong_dimensions() {
        let query = [1.0f32, 0.0];
        let vectors = vec![
            (10, vec![0.0, 1.0]),
            (11, vec![1.0, 0.0]),
            (12, vec![1.0, 1.0]),
            (13, vec![1.0]), // old model dimension
        ];
        let (ids, usable) = cosine_top_k(&query, &vectors, 2);
        assert_eq!(ids, vec![11, 12]);
        assert_eq!(usable, 3, "the mismatched vector is not usable");
    }

    #[test]
    fn cosine_handles_zero_queries_and_zero_vectors() {
        let vectors = vec![(1, vec![1.0, 0.0])];
        assert_eq!(cosine_top_k(&[0.0, 0.0], &vectors, 10), (Vec::new(), 0));
        assert_eq!(
            cosine_top_k(&[1.0, 0.0], &[(1, vec![0.0, 0.0])], 10),
            (Vec::new(), 1)
        );
    }

    #[test]
    fn status_messages_name_the_missing_setting() {
        let mut config = AiConfig {
            enabled: true,
            provider: ProviderKind::OpenAiCompatible,
            api_key: "sk".into(),
            ..AiConfig::default()
        };
        assert!(status_message(&config).unwrap().contains("embedding model"));
        config.embedding_model = "embed".into();
        assert!(status_message(&config).is_none());

        config.provider = ProviderKind::Anthropic;
        assert!(status_message(&config).unwrap().contains("Anthropic"));
    }
}
