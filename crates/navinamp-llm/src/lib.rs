//! Minimal LLM chat client for AI search.
//!
//! Two wire shapes cover every provider we support:
//! * **OpenAI-compatible** (`POST {base}/chat/completions`, bearer auth) —
//!   OpenRouter, OpenAI, DeepSeek, Zhipu GLM, Moonshot Kimi, MiniMax and
//!   local Ollama/LM Studio.
//! * **Anthropic** (`POST {base}/v1/messages`, `x-api-key`) — Claude.
//!
//! Tool use is deliberately *not* provider-native: the search orchestrator
//! (in the Tauri layer) asks the model to emit a JSON tool request and loops
//! in Rust. That keeps one prompt path for every provider, including models
//! with no function-calling support.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const USER_AGENT: &str = "NaVinaMP/0.9 (+https://github.com/prop/navinamp)";
const MAX_TOKENS: u32 = 1500;

/// Which API shape a provider speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ProviderKind {
    #[default]
    OpenAiCompatible,
    Anthropic,
}

/// One provider configuration, persisted by the app (`ai.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LlmConfig {
    /// Preset id from the UI (`openrouter`, `openai`, `anthropic`, …).
    pub preset: String,
    pub provider: ProviderKind,
    /// Base URL without the endpoint suffix, e.g. `https://openrouter.ai/api/v1`.
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    /// Per-request timeout; tool loops add one request per hop.
    pub timeout_secs: u64,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            preset: "openrouter".into(),
            provider: ProviderKind::OpenAiCompatible,
            base_url: "https://openrouter.ai/api/v1".into(),
            model: "deepseek/deepseek-v4.1-flash-1017".into(),
            api_key: String::new(),
            timeout_secs: 30,
        }
    }
}

/// Chat role in the provider-independent message list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("provider request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("provider returned HTTP {0}: {1}")]
    Status(u16, String),
    #[error("unexpected provider response: {0}")]
    Shape(String),
    #[error("provider returned an empty reply")]
    Empty,
}

pub type Result<T> = std::result::Result<T, LlmError>;

fn endpoint(config: &LlmConfig) -> (String, &'static str) {
    let base = config.base_url.trim_end_matches('/');
    match config.provider {
        ProviderKind::OpenAiCompatible => (format!("{base}/chat/completions"), "openai"),
        ProviderKind::Anthropic => (format!("{base}/v1/messages"), "anthropic"),
    }
}

fn openai_body(config: &LlmConfig, messages: &[ChatMessage]) -> Value {
    let messages: Vec<Value> = messages
        .iter()
        .map(|m| {
            json!({
                "role": match m.role {
                    Role::System => "system",
                    Role::User => "user",
                    Role::Assistant => "assistant",
                },
                "content": m.content,
            })
        })
        .collect();
    json!({
        "model": config.model,
        "messages": messages,
        "max_tokens": MAX_TOKENS,
        "temperature": 0.2,
    })
}

fn anthropic_body(config: &LlmConfig, messages: &[ChatMessage]) -> Value {
    // Anthropic takes the system prompt out of the message list.
    let system = messages
        .iter()
        .filter(|m| m.role == Role::System)
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    let messages: Vec<Value> = messages
        .iter()
        .filter(|m| m.role != Role::System)
        .map(|m| {
            json!({
                "role": match m.role {
                    Role::Assistant => "assistant",
                    _ => "user",
                },
                "content": m.content,
            })
        })
        .collect();
    json!({
        "model": config.model,
        "max_tokens": MAX_TOKENS,
        "system": system,
        "messages": messages,
        "temperature": 0.2,
    })
}

/// Parse an OpenAI-compatible `/chat/completions` response body.
fn parse_openai(body: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Response {
        #[serde(default)]
        choices: Vec<Choice>,
    }
    #[derive(Deserialize)]
    struct Choice {
        message: Message,
    }
    #[derive(Deserialize)]
    struct Message {
        content: Option<Value>,
    }
    let parsed: Response =
        serde_json::from_str(body).map_err(|e| LlmError::Shape(e.to_string()))?;
    let content = parsed
        .choices
        .into_iter()
        .next()
        .and_then(|choice| choice.message.content)
        .and_then(|value| match value {
            Value::String(text) => Some(text),
            Value::Array(parts) => {
                // Some gateways return content parts even for text replies.
                let text: String = parts
                    .into_iter()
                    .filter_map(|part| part.get("text").and_then(Value::as_str).map(str::to_owned))
                    .collect();
                (!text.is_empty()).then_some(text)
            }
            _ => None,
        })
        .ok_or(LlmError::Empty)?;
    Ok(content)
}

/// Parse an Anthropic `/v1/messages` response body (joins text blocks).
fn parse_anthropic(body: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Response {
        #[serde(default)]
        content: Vec<Block>,
    }
    #[derive(Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum Block {
        Text {
            #[serde(default)]
            text: String,
        },
        #[serde(other)]
        Other,
    }
    let parsed: Response =
        serde_json::from_str(body).map_err(|e| LlmError::Shape(e.to_string()))?;
    let text: String = parsed
        .content
        .into_iter()
        .filter_map(|block| match block {
            Block::Text { text } => Some(text),
            Block::Other => None,
        })
        .collect();
    if text.is_empty() {
        Err(LlmError::Empty)
    } else {
        Ok(text)
    }
}

/// Send one chat request and return the assistant text.
pub async fn chat(config: &LlmConfig, messages: &[ChatMessage]) -> Result<String> {
    if config.model.trim().is_empty() {
        return Err(LlmError::Shape("model is empty".into()));
    }
    let timeout = Duration::from_secs(config.timeout_secs.clamp(5, 300));
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .user_agent(USER_AGENT)
        .build()?;
    let (url, kind) = endpoint(config);
    let request = match config.provider {
        ProviderKind::OpenAiCompatible => {
            let mut request = client.post(&url).json(&openai_body(config, messages));
            // Local OpenAI-compatible servers (Ollama/LM Studio) often run
            // without a key; only send the header when one is configured.
            if !config.api_key.trim().is_empty() {
                request = request.bearer_auth(config.api_key.trim());
            }
            request
        }
        ProviderKind::Anthropic => client
            .post(&url)
            .header("x-api-key", config.api_key.trim())
            .header("anthropic-version", "2023-06-01")
            .json(&anthropic_body(config, messages)),
    };
    let response = request.send().await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        // Keep a short excerpt; the caller scrubs URLs before showing it.
        let excerpt: String = body.chars().take(400).collect();
        return Err(LlmError::Status(status.as_u16(), excerpt));
    }
    match kind {
        "openai" => parse_openai(&body),
        _ => parse_anthropic(&body),
    }
}

fn models_endpoint(config: &LlmConfig) -> String {
    let base = config.base_url.trim_end_matches('/');
    match config.provider {
        ProviderKind::OpenAiCompatible => format!("{base}/models"),
        ProviderKind::Anthropic => format!("{base}/v1/models"),
    }
}

/// Parse a model-list body. Accepts the OpenAI/Anthropic `{ "data": [...] }`
/// shape and the occasional `{ "models": [...] }` variant; entries may be
/// plain id strings or `{ "id": "…" }` objects.
fn parse_models(body: &str) -> Result<Vec<String>> {
    let value: Value = serde_json::from_str(body).map_err(|e| LlmError::Shape(e.to_string()))?;
    let list = value
        .get("data")
        .or_else(|| value.get("models"))
        .and_then(Value::as_array)
        .ok_or_else(|| LlmError::Shape("model list has no data array".into()))?;
    let mut ids: Vec<String> = list
        .iter()
        .filter_map(|item| match item {
            Value::String(id) => Some(id.trim().to_owned()),
            Value::Object(_) => item
                .get("id")
                .and_then(Value::as_str)
                .map(|id| id.trim().to_owned()),
            _ => None,
        })
        .filter(|id| !id.is_empty())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Err(LlmError::Empty);
    }
    Ok(ids)
}

/// Fetch the provider's available model ids for the settings picker.
pub async fn list_models(config: &LlmConfig) -> Result<Vec<String>> {
    let timeout = Duration::from_secs(config.timeout_secs.clamp(5, 300));
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .user_agent(USER_AGENT)
        .build()?;
    let url = models_endpoint(config);
    let request = match config.provider {
        ProviderKind::OpenAiCompatible => {
            let mut request = client.get(&url);
            if !config.api_key.trim().is_empty() {
                request = request.bearer_auth(config.api_key.trim());
            }
            request
        }
        ProviderKind::Anthropic => client
            .get(&url)
            .header("x-api-key", config.api_key.trim())
            .header("anthropic-version", "2023-06-01"),
    };
    let response = request.send().await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        let excerpt: String = body.chars().take(400).collect();
        return Err(LlmError::Status(status.as_u16(), excerpt));
    }
    parse_models(&body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(provider: ProviderKind, base: &str) -> LlmConfig {
        LlmConfig {
            provider,
            base_url: base.into(),
            model: "test-model".into(),
            api_key: "sk-secret".into(),
            ..LlmConfig::default()
        }
    }

    #[test]
    fn endpoints_trim_trailing_slashes_and_pick_the_api_shape() {
        assert_eq!(
            endpoint(&cfg(ProviderKind::OpenAiCompatible, "https://x.test/v1/")).0,
            "https://x.test/v1/chat/completions"
        );
        assert_eq!(
            endpoint(&cfg(ProviderKind::Anthropic, "https://api.anthropic.com")).0,
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn anthropic_hoists_the_system_prompt() {
        let body = anthropic_body(
            &cfg(ProviderKind::Anthropic, "https://a.test"),
            &[
                ChatMessage::system("rules"),
                ChatMessage::user("hello"),
                ChatMessage::assistant("hi"),
            ],
        );
        assert_eq!(body["system"], "rules");
        assert_eq!(body["messages"].as_array().unwrap().len(), 2);
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][1]["role"], "assistant");
    }

    #[test]
    fn parses_openai_string_and_part_arrays() {
        let plain = r#"{"choices":[{"message":{"content":"{\"ok\":true}"}}]}"#;
        assert_eq!(parse_openai(plain).unwrap(), r#"{"ok":true}"#);
        let parts = r#"{"choices":[{"message":{"content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]}}]}"#;
        assert_eq!(parse_openai(parts).unwrap(), "ab");
        assert!(matches!(
            parse_openai(r#"{"choices":[{"message":{}}]}"#),
            Err(LlmError::Empty)
        ));
    }

    #[test]
    fn parses_anthropic_text_blocks_and_ignores_others() {
        let body = r#"{"content":[{"type":"text","text":"a"},{"type":"tool_use","id":"t"},{"type":"text","text":"b"}]}"#;
        assert_eq!(parse_anthropic(body).unwrap(), "ab");
        assert!(matches!(
            parse_anthropic(r#"{"content":[]}"#),
            Err(LlmError::Empty)
        ));
    }

    #[tokio::test]
    async fn openai_chat_posts_to_the_compatible_endpoint() {
        use wiremock::matchers::{body_string_contains, header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header("authorization", "Bearer sk-secret"))
            .and(body_string_contains("\"model\":\"test-model\""))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"choices":[{"message":{"content":"hello"}}]}"#),
            )
            .mount(&server)
            .await;

        let config = LlmConfig {
            base_url: format!("{}/v1", server.uri()),
            ..cfg(ProviderKind::OpenAiCompatible, "")
        };
        let reply = chat(&config, &[ChatMessage::user("hi")]).await.unwrap();
        assert_eq!(reply, "hello");
    }

    #[tokio::test]
    async fn anthropic_chat_posts_messages_with_the_key_header() {
        use wiremock::matchers::{body_string_contains, header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("x-api-key", "sk-secret"))
            .and(header("anthropic-version", "2023-06-01"))
            .and(body_string_contains("\"system\":\"rules\""))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"content":[{"type":"text","text":"hello"}]}"#),
            )
            .mount(&server)
            .await;

        let config = LlmConfig {
            base_url: server.uri(),
            ..cfg(ProviderKind::Anthropic, "")
        };
        let reply = chat(
            &config,
            &[ChatMessage::system("rules"), ChatMessage::user("hi")],
        )
        .await
        .unwrap();
        assert_eq!(reply, "hello");
    }

    #[test]
    fn openai_body_uses_roles() {
        let body = openai_body(
            &cfg(ProviderKind::OpenAiCompatible, "https://x.test"),
            &[ChatMessage::system("s"), ChatMessage::user("u")],
        );
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["model"], "test-model");
    }

    #[test]
    fn parses_model_lists_in_both_shapes() {
        let openai = r#"{"data":[{"id":"gpt-5.2"},{"id":"gpt-5.1"},{"id":"gpt-5.1"}]}"#;
        assert_eq!(
            parse_models(openai).unwrap(),
            vec!["gpt-5.1".to_string(), "gpt-5.2".to_string()]
        );
        let plain = r#"{"models":["a","b"]}"#;
        assert_eq!(
            parse_models(plain).unwrap(),
            vec!["a".to_string(), "b".to_string()]
        );
        assert!(matches!(
            parse_models(r#"{"data":[]}"#),
            Err(LlmError::Empty)
        ));
        assert!(parse_models("{}").is_err());
    }

    #[tokio::test]
    async fn openai_model_list_uses_the_models_endpoint() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("authorization", "Bearer sk-secret"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"data":[{"id":"gpt-5.2"},{"id":"gpt-5.1"}]}"#),
            )
            .mount(&server)
            .await;

        let config = LlmConfig {
            base_url: format!("{}/v1", server.uri()),
            ..cfg(ProviderKind::OpenAiCompatible, "")
        };
        assert_eq!(
            list_models(&config).await.unwrap(),
            vec!["gpt-5.1".to_string(), "gpt-5.2".to_string()]
        );
    }

    #[tokio::test]
    async fn anthropic_model_list_uses_v1_models() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("x-api-key", "sk-secret"))
            .and(header("anthropic-version", "2023-06-01"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"data":[{"id":"claude-sonnet-5","display_name":"Claude Sonnet 5"}]}"#,
            ))
            .mount(&server)
            .await;

        let config = LlmConfig {
            base_url: server.uri(),
            ..cfg(ProviderKind::Anthropic, "")
        };
        assert_eq!(
            list_models(&config).await.unwrap(),
            vec!["claude-sonnet-5".to_string()]
        );
    }
}
