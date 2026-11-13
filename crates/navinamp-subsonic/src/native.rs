//! Navidrome's native (non-Subsonic) REST API: per-track tag data the
//! Subsonic surface cannot carry.
//!
//! The Subsonic `song` object has no label or catalog-number fields, and
//! OpenSubsonic only aggregates `recordLabels` at album level — Navidrome
//! does not aggregate `catalogNumber` at all. Navidrome's own API exposes
//! both per track (`catalogNum` field + `tags.recordlabel`), parsed from
//! the files' own tags. The web app authenticates with a JWT from
//! `POST /auth/login`; the Subsonic token does not work there, so the
//! plaintext password is required.
//!
//! Navidrome-only by nature: every other Subsonic-compatible server fails
//! here, and callers are expected to proceed without tags.

use std::collections::HashMap;

use serde::Deserialize;

use crate::{SubsonicClient, SubsonicError};

/// Songs per `/api/song` page. A ~20k-track library is ~30 MB as one
/// response; 2000 per page keeps each around 3 MB.
const PAGE: u64 = 2000;

/// Tag key Navidrome uses for record labels in the per-song `tags` map
/// (tag keys are lower-cased by the indexer).
const RECORD_LABEL_TAG: &str = "recordlabel";

/// Per-track tag data only the native API exposes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NativeTrackTags {
    /// The file's catalog-number tag (`catalogNum`), trimmed; `None` when
    /// the file carries none.
    pub catalog_num: Option<String>,
    /// The file's record-label tag(s) joined for display (`"A, B"`),
    /// trimmed; `None` when the file carries none.
    pub label: Option<String>,
}

/// One `/api/song` row — only the fields we need; serde skips the rest.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeSong {
    id: String,
    #[serde(default)]
    catalog_num: Option<String>,
    #[serde(default)]
    tags: HashMap<String, Vec<String>>,
}

impl SubsonicClient {
    /// Walk Navidrome's `/api/song` list and return per-track tags keyed by
    /// Subsonic song id. `library_id` is the music folder id; `None` walks
    /// every library on the server.
    ///
    /// Fails with [`SubsonicError::Auth`] when the login is rejected and
    /// [`SubsonicError::UnexpectedStatus`] on servers without the native
    /// API (404) — callers should treat any error as "no tags available".
    pub async fn native_track_tags(
        &self,
        password: &str,
        library_id: Option<&str>,
    ) -> Result<HashMap<String, NativeTrackTags>, SubsonicError> {
        let jwt = self.native_login(password).await?;
        let mut map = HashMap::new();
        let mut start = 0u64;
        loop {
            let mut url = self.native_url("song")?;
            {
                let mut pairs = url.query_pairs_mut();
                pairs.append_pair("_start", &start.to_string());
                pairs.append_pair("_end", &(start + PAGE).to_string());
                // Stable order: offset pagination over an unordered scan can
                // duplicate or skip rows while the server re-indexes.
                pairs.append_pair("_sort", "id");
                pairs.append_pair("_order", "ASC");
                if let Some(library) = library_id {
                    pairs.append_pair("library_id", library);
                }
            }
            let page: Vec<NativeSong> = self
                .http
                .get(url)
                .header("x-nd-authorization", format!("Bearer {jwt}"))
                .send()
                .await?
                .error_for_status()
                .map_err(native_status_error)?
                .json()
                .await?;
            let fetched = page.len() as u64;
            for song in page {
                let NativeSong {
                    id,
                    catalog_num,
                    tags,
                } = song;
                map.insert(id, native_tags_from(catalog_num, tags));
            }
            // The page size is a server-side default for Navidrome, so a
            // short page reliably marks the end of the list.
            if fetched < PAGE {
                return Ok(map);
            }
            start += PAGE;
        }
    }

    /// JWT login (`POST /auth/login`). Navidrome mounts authentication at
    /// the server root — deliberately *not* under the native `/api` prefix
    /// (`consts.URLPathNativeAPI` covers only the read API; the UI likewise
    /// calls `baseUrl('/auth/login')`).
    async fn native_login(&self, password: &str) -> Result<String, SubsonicError> {
        #[derive(Deserialize)]
        struct LoginResponse {
            #[serde(default)]
            token: Option<String>,
        }

        let url = self.server_root()?.join("auth/login")?;
        let body = serde_json::json!({ "username": self.username, "password": password });
        let resp = self
            .http
            .post(url)
            .json(&body)
            .send()
            .await?
            .error_for_status()
            .map_err(native_status_error)?;
        let parsed: LoginResponse = resp.json().await?;
        parsed.token.ok_or(SubsonicError::Auth)
    }

    /// Server root URL — the base minus its mandatory `rest/` suffix
    /// (`normalize_base_url` always leaves the base ending in `rest/`).
    fn server_root(&self) -> Result<url::Url, SubsonicError> {
        let root = self
            .base
            .as_str()
            .strip_suffix("rest/")
            .ok_or_else(|| SubsonicError::Url(url::ParseError::RelativeUrlWithoutBase))?;
        Ok(url::Url::parse(root)?)
    }

    /// Absolute URL for a native-API path under `/api`.
    fn native_url(&self, path: &str) -> Result<url::Url, SubsonicError> {
        Ok(self.server_root()?.join("api/")?.join(path)?)
    }
}

/// Map native-API status codes to the closest Subsonic error: the login
/// endpoint answers 401 for wrong credentials instead of the Subsonic
/// envelope's error code 40, so a rejected password still reads as
/// [`SubsonicError::Auth`]; other non-2xx statuses (404 on servers without
/// the native API) become [`SubsonicError::UnexpectedStatus`] like the
/// Subsonic layer's own non-2xx handling.
fn native_status_error(err: reqwest::Error) -> SubsonicError {
    match err.status() {
        Some(reqwest::StatusCode::UNAUTHORIZED) => SubsonicError::Auth,
        Some(status) => SubsonicError::UnexpectedStatus(status.as_u16()),
        None => SubsonicError::from(err),
    }
}

/// Collapse one native song row's tag fields into what's worth storing.
fn native_tags_from(
    catalog_num: Option<String>,
    tags: HashMap<String, Vec<String>>,
) -> NativeTrackTags {
    let label = match tags.get(RECORD_LABEL_TAG) {
        Some(labels) => {
            let joined = labels
                .iter()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            (!joined.is_empty()).then_some(joined)
        }
        None => None,
    };
    let catalog_num = catalog_num
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_owned);
    NativeTrackTags { catalog_num, label }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_json, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Enough minimal songs to fill exactly one page (see [`PAGE`]).
    fn page_of_ids(prefix: &str, from: u64, count: u64) -> serde_json::Value {
        let rows: Vec<serde_json::Value> = (from..from + count)
            .map(|i| serde_json::json!({ "id": format!("{prefix}{i}"), "name": "x" }))
            .collect();
        serde_json::Value::Array(rows)
    }

    #[tokio::test]
    async fn walks_pages_and_collects_tags() {
        let server = MockServer::start().await;
        let full: Vec<serde_json::Value> = (0..PAGE)
            .map(|i| {
                serde_json::json!({
                    "id": format!("s{i}"),
                    "title": "t",
                    "catalogNum": " NORD 3 ",
                    "tags": { "genre": ["Hardcore"], "recordlabel": ["Nordcore Records", " Second Label "] }
                })
            })
            .collect();
        // Full page, then a short one carrying the special shapes.
        let tail = serde_json::json!([
            { "id": "tail-plain", "title": "t" },
            { "id": "tail-empty", "title": "t", "catalogNum": "  ", "tags": { "recordlabel": ["  "] } }
        ]);
        Mock::given(method("POST"))
            .and(path("/auth/login"))
            .and(body_json(
                serde_json::json!({ "username": "u", "password": "p" }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "sess", "token": "jwt-token"
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/song"))
            .and(header("x-nd-authorization", "Bearer jwt-token"))
            .and(query_param("_start", "0"))
            .respond_with(ResponseTemplate::new(200).set_body_json(full))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/song"))
            .and(query_param("_start", PAGE.to_string()))
            .respond_with(ResponseTemplate::new(200).set_body_json(tail))
            .mount(&server)
            .await;

        let client = test_client(&server).await;
        let map = client.native_track_tags("p", None).await.expect("walk");

        assert_eq!(map.len() as u64, PAGE + 2);
        let tagged = &map["s0"];
        assert_eq!(tagged.catalog_num.as_deref(), Some("NORD 3"), "trimmed");
        assert_eq!(
            tagged.label.as_deref(),
            Some("Nordcore Records, Second Label")
        );
        assert_eq!(map["tail-plain"], NativeTrackTags::default());
        assert_eq!(map["tail-empty"], NativeTrackTags::default());
    }

    #[tokio::test]
    async fn forwards_library_filter() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/login"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "token": "jwt"
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/song"))
            .and(query_param("library_id", "6"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page_of_ids("s", 0, 1)))
            .mount(&server)
            .await;

        let client = test_client(&server).await;
        let map = client
            .native_track_tags("p", Some("6"))
            .await
            .expect("walk");
        assert_eq!(map["s0"], NativeTrackTags::default());
    }

    #[tokio::test]
    async fn login_rejection_maps_to_auth() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/login"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let client = test_client(&server).await;
        assert!(
            matches!(
                client.native_track_tags("p", None).await,
                Err(SubsonicError::Auth)
            ),
            "a 401 login must surface as the shared Auth error"
        );
    }

    #[tokio::test]
    async fn missing_native_api_maps_to_unexpected_status() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/login"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "token": "jwt"
            })))
            .mount(&server)
            .await;
        // wiremock answers unmatched requests with 404: a non-Navidrome
        // server without /api/song.
        let client = test_client(&server).await;
        assert!(
            matches!(
                client.native_track_tags("p", None).await,
                Err(SubsonicError::UnexpectedStatus(404))
            ),
            "servers without the native API must fail loudly, not silently"
        );
    }

    #[tokio::test]
    async fn honors_a_non_empty_server_base_path() {
        // Navidrome behind a reverse-proxy subpath: base …/nav/rest/ →
        // login at /nav/auth/login, songs at /nav/api/song.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/nav/rest/ping.view"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "subsonic-response": { "status": "ok", "version": "1.16.1" }
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/nav/auth/login"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "token": "jwt"
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/nav/api/song"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page_of_ids("s", 0, 1)))
            .mount(&server)
            .await;

        let base = format!("{}/nav/rest", server.uri());
        let client = SubsonicClient::connect(&base, "u", "seed-password", false)
            .await
            .expect("connect against the mocked subpath server");
        let map = client.native_track_tags("p", None).await.expect("walk");
        assert_eq!(map["s0"], NativeTrackTags::default());
    }

    /// A client pointed at the mock server's `/rest/` base. `connect` pings,
    /// so the shared ping mock lives here with the test setup.
    async fn test_client(server: &MockServer) -> SubsonicClient {
        Mock::given(method("GET"))
            .and(path("/rest/ping.view"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "subsonic-response": { "status": "ok", "version": "1.16.1" }
            })))
            .mount(server)
            .await;
        let base = format!("{}/rest", server.uri());
        SubsonicClient::connect(&base, "u", "seed-password", false)
            .await
            .expect("connect against the mocked server")
    }
}
