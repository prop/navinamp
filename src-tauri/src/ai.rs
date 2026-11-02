//! AI search: provider settings (`ai.json`), the prompt/tool loop, and the
//! optional Discogs catalog lookup.
//!
//! The model never receives file paths or API keys. It gets a compact
//! library digest (counts, genres, sampled artists, server ids) plus the
//! Smart View rule schema, and can answer with a Discogs catalog lookup, a
//! final Smart View definition, or both. Catalog entries are returned to the
//! UI directly; the library rules are validated and evaluated locally.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use navinamp_library::{LibraryStats, SmartViewDef};
use navinamp_llm::{ChatMessage, LlmConfig, ProviderKind};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Manager, State};

use crate::library::{LibraryState, with_library};
use crate::navidrome::NavidromeState;

const AI_FILE: &str = "ai.json";
const MAX_HOPS: usize = 6;
const MAX_TOOL_CALLS: usize = 4;
const MAX_CATALOG: usize = 400;
const CONTEXT_CHARS: usize = 10_000;
const USER_AGENT: &str = "NaVinaMP/0.9 (+https://github.com/prop/navinamp)";

/// Persisted AI settings. Plaintext, like `navidrome.json` (a keyring backend
/// is a future item); the UI labels the file location.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AiConfig {
    /// Show the AI search mode and allow calls.
    pub enabled: bool,
    /// Provider preset id (`openrouter`, `openai`, `anthropic`, `custom`, …).
    pub preset: String,
    pub provider: ProviderKind,
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub timeout_secs: u64,
    /// Optional Discogs personal access token (raises the rate limit).
    pub discogs_token: String,
}

impl Default for AiConfig {
    fn default() -> Self {
        let llm = LlmConfig::default();
        Self {
            enabled: false,
            preset: llm.preset,
            provider: llm.provider,
            base_url: llm.base_url,
            model: llm.model,
            api_key: llm.api_key,
            timeout_secs: llm.timeout_secs,
            discogs_token: String::new(),
        }
    }
}

impl AiConfig {
    fn llm(&self) -> LlmConfig {
        LlmConfig {
            preset: self.preset.clone(),
            provider: self.provider,
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            api_key: self.api_key.clone(),
            timeout_secs: self.timeout_secs,
        }
    }

    fn sanitized(mut self) -> Self {
        self.base_url = self.base_url.trim().trim_end_matches('/').to_owned();
        self.model = self.model.trim().to_owned();
        self.api_key = self.api_key.trim().to_owned();
        self.discogs_token = self.discogs_token.trim().to_owned();
        self.timeout_secs = self.timeout_secs.clamp(5, 300);
        self
    }

    fn scrubbed(&self, message: impl std::fmt::Display) -> String {
        let mut text = message.to_string();
        for secret in [&self.api_key, &self.discogs_token] {
            if !secret.is_empty() {
                text = text.replace(secret, "[redacted]");
            }
        }
        crate::navidrome::scrub_urls(&text)
    }
}

fn config_file(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("no config dir: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create `{}`: {e}", dir.display()))?;
    Ok(dir.join(AI_FILE))
}

fn load_config(app: &AppHandle) -> AiConfig {
    config_file(app)
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|raw| serde_json::from_str::<AiConfig>(&raw).ok())
        .unwrap_or_default()
        .sanitized()
}

fn save_config(app: &AppHandle, config: &AiConfig) -> Result<AiConfig, String> {
    let config = config.clone().sanitized();
    let path = config_file(app)?;
    let json = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("cannot write `{}`: {e}", path.display()))?;
    Ok(config)
}

/// Current AI settings (the settings panel owns editing; keys never log).
#[tauri::command]
pub fn ai_get_config(app: AppHandle) -> AiConfig {
    load_config(&app)
}

/// Persist AI settings; returns the sanitized values actually stored.
#[tauri::command]
pub fn ai_set_config(app: AppHandle, config: AiConfig) -> Result<AiConfig, String> {
    save_config(&app, &config)
}

/// One tiny completion with the supplied draft settings. Does not save them.
#[tauri::command]
pub async fn ai_test(config: AiConfig) -> Result<String, String> {
    let config = config.sanitized();
    if config.api_key.is_empty() && config.provider == ProviderKind::Anthropic {
        return Err("API key is required".into());
    }
    let llm = config.llm();
    let reply = navinamp_llm::chat(
        &llm,
        &[
            ChatMessage::system("You are a connectivity probe. Answer with one word."),
            ChatMessage::user("Reply with the single word OK."),
        ],
    )
    .await
    .map_err(|e| config.scrubbed(e))?;
    Ok(reply.trim().chars().take(200).collect())
}

/// Fetch the configured provider's model ids for the settings picker. Uses
/// the same draft settings as TEST and does not save them.
#[tauri::command]
pub async fn ai_models(config: AiConfig) -> Result<Vec<String>, String> {
    let config = config.sanitized();
    navinamp_llm::list_models(&config.llm())
        .await
        .map_err(|e| config.scrubbed(e))
}

/// Structured Discogs search arguments. All filters are optional; the model
/// can combine them (e.g. `label=Nordcore` + `format=Vinyl`).
#[derive(Debug, Clone, Default)]
struct DiscogsQuery {
    q: String,
    artist: String,
    release_title: String,
    label: String,
    genre: String,
    style: String,
    year: String,
    format: String,
    country: String,
    kind: String,
    page: u32,
    per_page: u32,
}

impl DiscogsQuery {
    /// Lenient extraction from the model's JSON tool object. `type`/`kind`,
    /// `query`/`q`, `limit`/`per_page` aliases are accepted.
    fn from_value(value: &Value) -> Self {
        fn text(value: &Value, keys: &[&str]) -> String {
            keys.iter()
                .find_map(|key| value.get(*key))
                .map(|v| match v {
                    Value::String(s) => s.trim().to_owned(),
                    Value::Number(n) => n.to_string(),
                    _ => String::new(),
                })
                .unwrap_or_default()
        }
        fn number(value: &Value, keys: &[&str]) -> Option<u64> {
            keys.iter().find_map(|key| {
                value.get(*key).and_then(|v| {
                    v.as_u64()
                        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                })
            })
        }
        Self {
            q: text(value, &["q", "query"]),
            artist: text(value, &["artist"]),
            release_title: text(value, &["release_title", "releaseTitle", "title"]),
            label: text(value, &["label"]),
            genre: text(value, &["genre"]),
            style: text(value, &["style"]),
            year: text(value, &["year"]),
            format: text(value, &["format"]),
            country: text(value, &["country"]),
            kind: text(value, &["kind", "type"]),
            page: number(value, &["page"]).unwrap_or(1).clamp(1, 10_000) as u32,
            per_page: number(value, &["per_page", "perPage", "limit"])
                .unwrap_or(50)
                .clamp(1, 100) as u32,
        }
    }

    fn filters(&self) -> Vec<(&'static str, &str)> {
        let mut out = Vec::new();
        if !self.q.is_empty() {
            out.push(("q", self.q.as_str()));
        }
        if !self.artist.is_empty() {
            out.push(("artist", self.artist.as_str()));
        }
        if !self.release_title.is_empty() {
            out.push(("release_title", self.release_title.as_str()));
        }
        if !self.label.is_empty() {
            out.push(("label", self.label.as_str()));
        }
        if !self.genre.is_empty() {
            out.push(("genre", self.genre.as_str()));
        }
        if !self.style.is_empty() {
            out.push(("style", self.style.as_str()));
        }
        if !self.year.is_empty() {
            out.push(("year", self.year.as_str()));
        }
        if !self.format.is_empty() {
            out.push(("format", self.format.as_str()));
        }
        if !self.country.is_empty() {
            out.push(("country", self.country.as_str()));
        }
        out
    }

    fn is_empty(&self) -> bool {
        self.filters().is_empty()
    }

    fn describe(&self) -> String {
        let filters: Vec<String> = self
            .filters()
            .into_iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        if filters.is_empty() {
            "(empty)".into()
        } else {
            filters.join(", ")
        }
    }
}

/// One Discogs release, shown in the UI catalog and sent compactly to the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscogsRelease {
    pub id: u64,
    pub title: String,
    pub year: Option<String>,
    pub label: Vec<String>,
    pub format: Vec<String>,
    pub genre: Vec<String>,
    pub style: Vec<String>,
    pub country: String,
    pub kind: String,
}

impl DiscogsRelease {
    fn key(&self) -> String {
        if self.id != 0 {
            format!("#{}", self.id)
        } else {
            format!("{}|{}", self.title, self.year.as_deref().unwrap_or(""))
        }
    }
}

/// One tool-call outcome: full catalog entries for the UI and a compact
/// payload for the model.
struct DiscogsLookup {
    model_payload: String,
    releases: Vec<DiscogsRelease>,
    total: u32,
}

/// What the model may reply with.
enum Reply {
    Discogs(DiscogsQuery),
    Final {
        def: Option<SmartViewDef>,
        explanation: String,
    },
}

/// Result of a successful AI search. `def` is `None` for catalog-only answers;
/// the UI then shows `catalog` instead of a track query.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSearchReport {
    pub def: Option<SmartViewDef>,
    pub explanation: String,
    pub catalog: Vec<DiscogsRelease>,
    /// Total releases Discogs reported across the fetched pages (0 = unknown).
    pub catalog_total: u32,
    pub preset: String,
    pub model: String,
    pub tool_calls: u32,
}

/// Translate a free-form prompt (optionally refining `base_def`) into a
/// validated Smart View definition. Tool calls happen here, in Rust; the
/// provider only sees the compact query results.
#[tauri::command]
pub async fn ai_search(
    app: AppHandle,
    state: State<'_, LibraryState>,
    nav: State<'_, NavidromeState>,
    prompt: String,
    base_def: Option<SmartViewDef>,
) -> Result<AiSearchReport, String> {
    let clean = prompt.trim();
    if clean.is_empty() {
        return Err("enter a search prompt first".into());
    }
    let config = load_config(&app);
    if !config.enabled {
        return Err("AI search is disabled — configure it in AI SETTINGS".into());
    }
    if config.api_key.is_empty() && config.provider == ProviderKind::Anthropic {
        return Err("AI API key is missing".into());
    }

    let context = with_library(&app, &state, |library| {
        let stats = library.stats().map_err(|e| e.to_string())?;
        let genres = library.genres(None).map_err(|e| e.to_string())?;
        let artists = library.artists(None).map_err(|e| e.to_string())?;
        let servers = crate::navidrome::profile_names(&nav);
        Ok(build_context(&stats, &genres, &artists, &servers))
    })?;

    let mut messages = vec![
        ChatMessage::system(system_prompt()),
        ChatMessage::user(user_prompt(clean, &context, base_def.as_ref())),
    ];
    let llm = config.llm();
    let mut tool_calls = 0u32;
    let mut repairs = 0u8;
    let mut last_issue = String::new();
    let mut catalog: Vec<DiscogsRelease> = Vec::new();
    let mut catalog_keys: HashSet<String> = HashSet::new();
    let mut catalog_total = 0u32;

    for _ in 0..MAX_HOPS {
        let text = navinamp_llm::chat(&llm, &messages)
            .await
            .map_err(|e| config.scrubbed(e))?;
        match parse_reply(&text) {
            Ok(Reply::Discogs(query)) if tool_calls < MAX_TOOL_CALLS as u32 => {
                tool_calls += 1;
                let described = query.describe();
                match discogs_search(&config, &query).await {
                    Ok(lookup) => {
                        catalog_total = catalog_total.max(lookup.total);
                        for release in lookup.releases {
                            if catalog.len() >= MAX_CATALOG {
                                break;
                            }
                            if catalog_keys.insert(release.key()) {
                                catalog.push(release);
                            }
                        }
                        messages.push(ChatMessage::assistant(text));
                        messages.push(ChatMessage::user(format!(
                            "Discogs results for {described}:\n{}\n\n\
                             The UI already shows these releases to the user. Continue: \
                             request another page if the user asked for all and \
                             pagination.pages > pagination.page; otherwise finish. \
                             For a catalog-only answer set \"def\": null.",
                            lookup.model_payload
                        )));
                    }
                    Err(e) => {
                        messages.push(ChatMessage::assistant(text));
                        messages.push(ChatMessage::user(format!(
                            "Discogs lookup failed: {}. Continue with what you have; \
                             do not repeat the same filters.",
                            config.scrubbed(e)
                        )));
                    }
                }
            }
            Ok(Reply::Discogs(_)) => {
                messages.push(ChatMessage::assistant(text));
                messages.push(ChatMessage::user(
                    "Discogs lookups are exhausted. Finish now with the final JSON; \
                     use \"def\": null if this is a catalog-only answer.",
                ));
            }
            Ok(Reply::Final { def, explanation }) => match def {
                Some(mut def) => {
                    if def.name.trim().is_empty() {
                        def.name = base_def
                            .as_ref()
                            .map(|b| b.name.trim().to_owned())
                            .filter(|name| !name.is_empty())
                            .unwrap_or_else(|| default_name(clean));
                    }
                    def.limit = Some(def.limit.unwrap_or(1000.0).clamp(1.0, 5000.0));
                    // Validate the definition with the same engine the UI uses.
                    match with_library(&app, &state, |library| {
                        library.smart_view_query(&def, 1).map_err(|e| e.to_string())
                    }) {
                        Ok(_) => {
                            return Ok(AiSearchReport {
                                def: Some(def),
                                explanation,
                                catalog,
                                catalog_total,
                                preset: config.preset.clone(),
                                model: config.model.clone(),
                                tool_calls,
                            });
                        }
                        Err(e) if repairs == 0 => {
                            repairs = 1;
                            last_issue = e.clone();
                            messages.push(ChatMessage::assistant(text));
                            messages.push(ChatMessage::user(format!(
                                "That definition failed validation: {e}\n\
                                 Fix it and output the final JSON again. Use only the documented fields and operators."
                            )));
                        }
                        Err(e) if !catalog.is_empty() => {
                            // The catalog is still useful; return it without a
                            // broken local rule instead of failing the search.
                            let note = if explanation.trim().is_empty() {
                                format!(
                                    "Discogs catalog results. Local rules could not be validated: {e}"
                                )
                            } else {
                                format!("{explanation}\n\nLocal rules could not be validated: {e}")
                            };
                            return Ok(AiSearchReport {
                                def: None,
                                explanation: note,
                                catalog,
                                catalog_total,
                                preset: config.preset.clone(),
                                model: config.model.clone(),
                                tool_calls,
                            });
                        }
                        Err(e) => return Err(format!("AI produced an invalid query: {e}")),
                    }
                }
                None => {
                    if catalog.is_empty() {
                        return Err(
                            "AI returned no local rules and no Discogs catalog results".into()
                        );
                    }
                    let explanation = if explanation.trim().is_empty() {
                        "Discogs catalog results.".to_owned()
                    } else {
                        explanation
                    };
                    return Ok(AiSearchReport {
                        def: None,
                        explanation,
                        catalog,
                        catalog_total,
                        preset: config.preset.clone(),
                        model: config.model.clone(),
                        tool_calls,
                    });
                }
            },
            Err(e) => {
                last_issue = e;
                if repairs > 0 {
                    break;
                }
                repairs = 1;
                messages.push(ChatMessage::assistant(text));
                messages.push(ChatMessage::user(format!(
                    "That reply was invalid: {last_issue}\n\
                     Output ONLY one JSON object, no prose and no code fences."
                )));
            }
        }
    }
    if !catalog.is_empty() {
        return Ok(AiSearchReport {
            def: None,
            explanation: if last_issue.is_empty() {
                "Discogs catalog results (the model did not finish a local query).".to_owned()
            } else {
                format!(
                    "Discogs catalog results (the model did not finish a local query: {last_issue})."
                )
            },
            catalog,
            catalog_total,
            preset: config.preset.clone(),
            model: config.model.clone(),
            tool_calls,
        });
    }
    Err(format!(
        "AI did not produce a usable search query{}",
        if last_issue.is_empty() {
            String::new()
        } else {
            format!(" ({last_issue})")
        }
    ))
}

fn default_name(prompt: &str) -> String {
    prompt
        .chars()
        .take(60)
        .collect::<String>()
        .trim()
        .to_owned()
}

// ---------------------------------------------------------------------------
// Prompt construction
// ---------------------------------------------------------------------------

fn system_prompt() -> String {
    r#"You translate music requests into NaVinaMP results. Output ONE JSON object
and nothing else (no markdown, no code fences).

Two possible replies:

1. Ask the Discogs catalog (max 4 calls):
{"action":"discogs_search","q":"free text","artist":"","release_title":"","label":"","genre":"","style":"","year":"","format":"Vinyl","country":"","kind":"release|master|artist","page":1,"per_page":100}
Every key except action is optional; combine structured filters instead of
putting everything into q. The results are shown DIRECTLY to the user in a
Discogs catalog panel, so use this for label / discography / pressing / release
questions. If the user says "all", set per_page 100 and request the next page
while pagination.pages > pagination.page (up to 4 calls).

Example: "all Nordcore vinyl records according to Discogs"
{"action":"discogs_search","label":"Nordcore","format":"Vinyl","kind":"release","per_page":100}
Then finish with "def": null and an explanation, since the library has no
label tag. Do not collapse a catalog request into a broad keyword rule.

2. Finish:
{"action":"final","explanation":"one or two sentences","def": null}
or, when local tracks should also be returned:
{"action":"final","explanation":"...","def":{
  "name":"<short name>",
  "group":{"match":"all","rules":[{"field":"artist","op":"contains","value":"radiohead"}]},
  "limit":1000,
  "sort":"title",
  "sortDesc":false
}}

Use "def": null for catalog-only answers. When the user also wants local tracks,
build def from the catalog's artist/release titles (album/artist contains), not
just the label keyword. If the library cannot express a filter (e.g. it has no
label tag), say so in the explanation.

Rule schema:
- Text fields: title, artist, albumArtist, album, genre, codec, path, source, server.
  Operators: is, isNot, contains, notContains, startsWith, endsWith.
  Values are strings. `source` is "local" or "navidrome". `path` also matches
  navidrome://<server>/<id>; `server` is a profile id from the library digest.
- Number fields: year, rating, playCount, durationSecs, bitrateKbps.
  Operators: eq, ne, gt, gte, lt, lte, between (between needs value and value2).
  `rating` is 0-5. `durationSecs` is seconds.
- Boolean field: starred. Use is/isNot with true/false (1/0 also works).
- Date fields: lastPlayed, dateAdded.
  Operators: inLastDays, notInLastDays (numeric days), before, after (YYYY-MM-DD).
- `group.match` is "all" (AND) or "any" (OR). Rules are flat; no nesting.
- `sort`: title, artist, album, random, mostPlayed, recentlyAdded, recentlyPlayed.

Guidance:
- Prefer broad `contains`/`startsWith` over exact `is` for names.
- For mood/vibe requests ("rainy day jazz", "driving rock"), approximate with
  the available tags (genre, year, rating, playCount, starred) and say so in
  the explanation. Do not invent genres; use the library digest.
- "liked/favourite" means starred = 1. "recently added/played" use the date
  fields. "long songs" use durationSecs (e.g. gt 360).
- When refining an existing definition, keep its good rules and change only
  what the new instruction asks for.
"#
    .into()
}

fn user_prompt(prompt: &str, context: &str, base: Option<&SmartViewDef>) -> String {
    let mut out = format!(
        "Library digest:\n{context}\n\nUser request:\n{prompt}\n\n\
         Reply with the JSON object now."
    );
    if let Some(base) = base
        && let Ok(json) = serde_json::to_string(base)
    {
        out.push_str("\n\nExisting definition to refine:\n");
        out.push_str(&json);
        out.push_str("\nKeep the same name unless the instruction implies a new one.");
    }
    out
}

/// Compact, bounded digest of the library for the prompt. Artists are sampled
/// across the whole alphabet (not just A-C) to stay representative.
fn build_context(
    stats: &LibraryStats,
    genres: &[String],
    artists: &[String],
    servers: &[(String, String)],
) -> String {
    let mut out = format!(
        "{} tracks, {} albums, {} artists, {} genres. local={} tracks, navidrome={} tracks.\n",
        stats.tracks,
        stats.albums,
        stats.artists,
        stats.genres,
        stats.local.tracks,
        stats.navidrome.tracks
    );
    if !servers.is_empty() {
        out.push_str("Navidrome servers: ");
        let list: Vec<String> = servers
            .iter()
            .map(|(name, id)| {
                let count = stats
                    .navidrome_servers
                    .iter()
                    .find(|s| &s.server_id == id)
                    .map(|s| s.tracks)
                    .unwrap_or(0);
                format!("“{name}” (server id {id}, {count} tracks)")
            })
            .collect();
        out.push_str(&list.join(", "));
        out.push('\n');
    }
    out.push_str("Genres: ");
    out.push_str(&sample_join(genres, 120));
    out.push_str("\nArtists (sampled): ");
    out.push_str(&sample_join(artists, 250));
    out.push('\n');
    if out.chars().count() > CONTEXT_CHARS {
        out = out.chars().take(CONTEXT_CHARS).collect();
    }
    out
}

/// Join an evenly-spaced sample so long lists stay representative.
fn sample_join(items: &[String], max: usize) -> String {
    if items.is_empty() {
        return "(none)".into();
    }
    let step = items.len().div_ceil(max).max(1);
    items
        .iter()
        .step_by(step)
        .take(max)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// Reply parsing
// ---------------------------------------------------------------------------

/// Pull the first JSON object out of a model reply. Tolerates code fences and
/// a stray leading/trailing sentence, which weaker models occasionally add.
fn extract_json(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end < start {
        return None;
    }
    serde_json::from_str(&text[start..=end]).ok()
}

fn has_discogs_filters(value: &Value) -> bool {
    [
        "q",
        "query",
        "artist",
        "release_title",
        "releaseTitle",
        "title",
        "label",
        "genre",
        "style",
        "year",
        "format",
        "country",
        "kind",
        "type",
        "page",
        "per_page",
        "perPage",
        "limit",
    ]
    .iter()
    .any(|key| value.get(*key).is_some())
}

fn parse_reply(text: &str) -> Result<Reply, String> {
    let value = extract_json(text).ok_or_else(|| {
        format!(
            "response was not JSON: {}",
            text.chars().take(160).collect::<String>()
        )
    })?;
    let action = value
        .get("action")
        .or_else(|| value.get("tool"))
        .and_then(Value::as_str)
        .unwrap_or("");

    // A final answer is explicit, or carries a def/group object. `def` may be
    // null for catalog-only answers.
    if action == "final" || value.get("def").is_some() || value.get("group").is_some() {
        let def_value = value.get("def").cloned().or_else(|| {
            // Accept a bare definition object too.
            value.get("group").map(|_| value.clone())
        });
        let def = match def_value {
            None | Some(Value::Null) => None,
            Some(raw) => Some(
                serde_json::from_value::<SmartViewDef>(raw)
                    .map_err(|e| format!("invalid smart view definition: {e}"))?,
            ),
        };
        let explanation = value
            .get("explanation")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_owned();
        return Ok(Reply::Final { def, explanation });
    }

    if action == "discogs_search" || action == "discogs" || has_discogs_filters(&value) {
        let query = DiscogsQuery::from_value(&value);
        if query.is_empty() {
            return Err(
                "discogs_search needs at least one filter (q, label, artist, release_title, genre, style, year, format, country)"
                    .into(),
            );
        }
        return Ok(Reply::Discogs(query));
    }

    Err("expected a discogs_search or a final object".into())
}

// ---------------------------------------------------------------------------
// Discogs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
struct DiscogsResponse {
    #[serde(default)]
    results: Vec<DiscogsItem>,
    #[serde(default)]
    pagination: DiscogsPagination,
}

#[derive(Debug, Deserialize, Default)]
struct DiscogsPagination {
    #[serde(default)]
    page: u32,
    #[serde(default)]
    pages: u32,
    #[serde(default)]
    items: u32,
    #[serde(default, rename = "per_page")]
    per_page: u32,
}

#[derive(Debug, Deserialize, Default)]
struct DiscogsItem {
    #[serde(default)]
    id: Option<Value>,
    #[serde(default)]
    title: String,
    /// Discogs mixes numeric and string years across endpoints.
    #[serde(default)]
    year: Option<Value>,
    #[serde(default)]
    genre: Vec<String>,
    #[serde(default)]
    style: Vec<String>,
    #[serde(default)]
    label: Vec<String>,
    #[serde(default)]
    format: Vec<String>,
    #[serde(default)]
    country: String,
    #[serde(default, rename = "type")]
    kind: String,
}

fn release_from_item(item: DiscogsItem) -> DiscogsRelease {
    let id = match item.id {
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    };
    let year = item.year.and_then(|value| match value {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_owned()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    });
    DiscogsRelease {
        id,
        title: item.title,
        year,
        label: item.label,
        format: item.format,
        genre: item.genre,
        style: item.style,
        country: item.country,
        kind: item.kind,
    }
}

/// Search Discogs with structured filters. Full entries go to the UI catalog;
/// the model gets a compact projection plus pagination.
async fn discogs_search(config: &AiConfig, query: &DiscogsQuery) -> Result<DiscogsLookup, String> {
    if query.is_empty() {
        return Err("discogs_search needs at least one filter".into());
    }
    let kind = match query.kind.as_str() {
        "master" => "master",
        "artist" => "artist",
        _ => "release",
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| format!("discogs client: {e}"))?;
    let per_page = query.per_page.clamp(1, 100);
    let page = query.page.max(1);
    let mut params: Vec<(&str, String)> = vec![("type", kind.to_owned())];
    params.extend(
        query
            .filters()
            .into_iter()
            .map(|(key, value)| (key, value.to_owned())),
    );
    params.push(("per_page", per_page.to_string()));
    params.push(("page", page.to_string()));
    let mut request = client
        .get("https://api.discogs.com/database/search")
        .query(&params);
    if !config.discogs_token.is_empty() {
        request = request.header(
            "Authorization",
            format!("Discogs token={}", config.discogs_token),
        );
    }
    let response = request
        .send()
        .await
        .map_err(|e| format!("discogs request: {e}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("discogs body: {e}"))?;
    if !status.is_success() {
        return Err(format!(
            "discogs returned HTTP {}: {}",
            status.as_u16(),
            body.chars().take(300).collect::<String>()
        ));
    }
    let parsed: DiscogsResponse =
        serde_json::from_str(&body).map_err(|e| format!("discogs response: {e}"))?;
    let total = if parsed.pagination.items > 0 {
        parsed.pagination.items
    } else {
        parsed.results.len() as u32
    };
    let releases: Vec<DiscogsRelease> = parsed.results.into_iter().map(release_from_item).collect();
    let model_results: Vec<Value> = releases
        .iter()
        .map(|release| {
            json!({
                "title": release.title,
                "year": release.year,
                "label": release.label.first(),
                "format": release.format.join("/"),
                "genre": release.genre,
                "style": release.style,
                "country": release.country,
                "type": release.kind,
            })
        })
        .collect();
    let response_page = if parsed.pagination.page > 0 {
        parsed.pagination.page
    } else {
        page
    };
    let response_per_page = if parsed.pagination.per_page > 0 {
        parsed.pagination.per_page
    } else {
        per_page
    };
    let payload = json!({
        "pagination": {
            "page": response_page,
            "pages": parsed.pagination.pages,
            "items": total,
            "per_page": response_per_page,
        },
        "results": model_results,
    });
    Ok(DiscogsLookup {
        model_payload: payload.to_string(),
        releases,
        total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_json_from_fences_and_prose() {
        let value = extract_json("Sure!\n```json\n{\"action\":\"final\",\"def\":{}}\n```").unwrap();
        assert_eq!(value["action"], "final");
    }

    #[test]
    fn parses_discogs_and_final_replies() {
        match parse_reply(
            r#"{"action":"discogs_search","query":"blue note","kind":"release","limit":5}"#,
        )
        .unwrap()
        {
            Reply::Discogs(query) => {
                assert_eq!(query.q, "blue note");
                assert_eq!(query.kind, "release");
                assert_eq!(query.per_page, 5);
                assert_eq!(query.page, 1);
            }
            _ => panic!("expected discogs"),
        }
        let final_json = r#"{
            "action":"final",
            "explanation":"jazz from the 90s",
            "def":{
                "name":"90s jazz",
                "group":{"match":"all","rules":[
                    {"field":"genre","op":"contains","value":"jazz"},
                    {"field":"year","op":"between","value":1990,"value2":1999}
                ]},
                "limit":500,
                "sort":"title",
                "sortDesc":false
            }
        }"#;
        match parse_reply(final_json).unwrap() {
            Reply::Final { def, explanation } => {
                let def = def.expect("local def");
                assert_eq!(def.name, "90s jazz");
                assert_eq!(def.group.rules.len(), 2);
                assert_eq!(explanation, "jazz from the 90s");
            }
            _ => panic!("expected final"),
        }
    }

    #[test]
    fn parses_structured_discogs_filters_and_pages() {
        let reply = parse_reply(
            r#"{"action":"discogs_search","label":"Nordcore","format":"Vinyl","type":"release","page":2,"per_page":100}"#,
        )
        .unwrap();
        match reply {
            Reply::Discogs(query) => {
                assert_eq!(query.label, "Nordcore");
                assert_eq!(query.format, "Vinyl");
                assert_eq!(query.kind, "release");
                assert_eq!(query.page, 2);
                assert_eq!(query.per_page, 100);
                assert_eq!(
                    query.filters(),
                    vec![("label", "Nordcore"), ("format", "Vinyl")]
                );
            }
            _ => panic!("expected discogs"),
        }
    }

    #[test]
    fn final_with_null_def_is_catalog_only() {
        match parse_reply(r#"{"action":"final","explanation":"the catalog is above","def":null}"#)
            .unwrap()
        {
            Reply::Final { def, explanation } => {
                assert!(def.is_none());
                assert_eq!(explanation, "the catalog is above");
            }
            _ => panic!("expected final"),
        }
    }

    #[test]
    fn bare_definition_is_accepted() {
        let bare = r#"{
            "name":"rock",
            "group":{"match":"all","rules":[{"field":"genre","op":"is","value":"rock"}]},
            "limit":null,
            "sort":"artist",
            "sortDesc":false
        }"#;
        assert!(matches!(
            parse_reply(bare),
            Ok(Reply::Final { def: Some(_), .. })
        ));
    }

    #[test]
    fn discogs_items_normalize_ids_and_years() {
        let release = release_from_item(DiscogsItem {
            id: Some(json!("123")),
            title: "Artist - Album".into(),
            year: Some(json!(1999)),
            label: vec!["Label".into()],
            format: vec!["Vinyl".into(), "LP".into()],
            ..DiscogsItem::default()
        });
        assert_eq!(release.id, 123);
        assert_eq!(release.year.as_deref(), Some("1999"));
        assert_eq!(release.key(), "#123");
    }

    #[test]
    fn rejects_prose_only() {
        assert!(parse_reply("I could not find anything.").is_err());
    }

    #[test]
    fn samples_are_bounded_and_spread() {
        let items: Vec<String> = (0..1000).map(|i| format!("item{i}")).collect();
        let sample = sample_join(&items, 10);
        assert_eq!(sample.split(", ").count(), 10);
        assert!(sample.contains("item0"));
        assert!(sample.contains("item900"));
    }
}
