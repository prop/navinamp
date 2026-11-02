//! AI search: provider settings (`ai.json`), the prompt/tool loop, and the
//! optional Discogs catalog lookup.
//!
//! The model never receives file paths or API keys. It gets a compact
//! library digest (counts, genres, sampled artists, server ids) plus the
//! Smart View rule schema, and answers with either
//! `{"action":"discogs_search",…}` or a final Smart View definition. The
//! definition is validated by the library and evaluated locally, so the
//! resulting rows are deterministic and can be saved as a smart view.

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
const MAX_HOPS: usize = 4;
const MAX_TOOL_CALLS: usize = 2;
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

/// What the model may reply with.
enum Reply {
    Discogs {
        query: String,
        kind: String,
        limit: u32,
    },
    Final {
        def: SmartViewDef,
        explanation: String,
    },
}

/// Result of a successful AI search. The UI runs `smart_view_query` with
/// `def` (the normal advanced-search path) to fetch the rows.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSearchReport {
    pub def: SmartViewDef,
    pub explanation: String,
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

    for _ in 0..MAX_HOPS {
        let text = navinamp_llm::chat(&llm, &messages)
            .await
            .map_err(|e| config.scrubbed(e))?;
        match parse_reply(&text) {
            Ok(Reply::Discogs { query, kind, limit }) if tool_calls < MAX_TOOL_CALLS as u32 => {
                tool_calls += 1;
                let results = match discogs_search(&config, &query, &kind, limit).await {
                    Ok(results) => results,
                    Err(e) => format!(
                        "Discogs lookup failed: {}. Continue without it and emit final rules.",
                        config.scrubbed(e)
                    ),
                };
                messages.push(ChatMessage::assistant(text));
                messages.push(ChatMessage::user(format!(
                    "Discogs results for “{query}”:\n{results}\n\n\
                     Continue. Output another discogs_search JSON if needed, otherwise the final JSON."
                )));
            }
            Ok(Reply::Discogs { .. }) => {
                messages.push(ChatMessage::assistant(text));
                messages.push(ChatMessage::user(
                    "Discogs lookups are exhausted. Output the final JSON now.",
                ));
            }
            Ok(Reply::Final {
                mut def,
                explanation,
            }) => {
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
                            def,
                            explanation,
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
                    Err(e) => return Err(format!("AI produced an invalid query: {e}")),
                }
            }
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
    r#"You translate music-library requests into NaVinaMP Smart View rules.

Output ONE JSON object and nothing else (no markdown, no code fences).

Two possible replies:

1. Ask Discogs for catalog knowledge (max 2 calls per search):
{"action":"discogs_search","query":"<search terms>","kind":"release|master|artist","limit":10}
Use this when the answer depends on information that is not in library tags: labels, producers, stylistic eras, original release years, "the album with the blue cover", etc. The result arrives as compact JSON (title, year, genre, style, label, format).

2. Finish with a Smart View definition:
{"action":"final","explanation":"one or two sentences","def":{
  "name":"<short name>",
  "group":{"match":"all","rules":[{"field":"artist","op":"contains","value":"radiohead"}]},
  "limit":1000,
  "sort":"title",
  "sortDesc":false
}}

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

    if action == "discogs_search" || action == "discogs" || value.get("query").is_some() {
        let query = value
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if query.is_empty() {
            return Err("discogs_search needs a query".into());
        }
        let kind = value
            .get("kind")
            .or_else(|| value.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("release");
        let limit = value
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(10)
            .clamp(1, 25) as u32;
        return Ok(Reply::Discogs {
            query: query.to_owned(),
            kind: kind.to_owned(),
            limit,
        });
    }

    let def_value = value.get("def").cloned().or_else(|| {
        // Accept a bare definition object too.
        value.get("group").map(|_| value.clone())
    });
    let Some(def_value) = def_value else {
        return Err("expected a discogs_search or a final `def` object".into());
    };
    let def: SmartViewDef = serde_json::from_value(def_value)
        .map_err(|e| format!("invalid smart view definition: {e}"))?;
    let explanation = value
        .get("explanation")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    Ok(Reply::Final { def, explanation })
}

// ---------------------------------------------------------------------------
// Discogs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct DiscogsResponse {
    #[serde(default)]
    results: Vec<DiscogsItem>,
}

#[derive(Debug, Deserialize)]
struct DiscogsItem {
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
    #[serde(default, rename = "type")]
    kind: String,
}

/// Search Discogs and return a compact JSON array for the model. The token is
/// optional; authenticated calls get the higher rate limit.
async fn discogs_search(
    config: &AiConfig,
    query: &str,
    kind: &str,
    limit: u32,
) -> Result<String, String> {
    let kind = match kind {
        "master" => "master",
        "artist" => "artist",
        _ => "release",
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| format!("discogs client: {e}"))?;
    let per_page = limit.clamp(1, 25).to_string();
    let mut request = client
        .get("https://api.discogs.com/database/search")
        .query(&[
            ("q", query),
            ("type", kind),
            ("per_page", per_page.as_str()),
        ]);
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
    let items: Vec<Value> = parsed
        .results
        .into_iter()
        .take(limit as usize)
        .map(|item| {
            json!({
                "title": item.title,
                "year": item.year,
                "type": item.kind,
                "genre": item.genre,
                "style": item.style,
                "label": item.label,
                "format": item.format,
            })
        })
        .collect();
    serde_json::to_string(&items).map_err(|e| format!("discogs summary: {e}"))
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
            Reply::Discogs { query, kind, limit } => {
                assert_eq!(query, "blue note");
                assert_eq!(kind, "release");
                assert_eq!(limit, 5);
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
                assert_eq!(def.name, "90s jazz");
                assert_eq!(def.group.rules.len(), 2);
                assert_eq!(explanation, "jazz from the 90s");
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
        assert!(matches!(parse_reply(bare), Ok(Reply::Final { .. })));
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
