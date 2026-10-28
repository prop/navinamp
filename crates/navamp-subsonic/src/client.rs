//! The Subsonic/OpenSubsonic HTTP client.

use std::fmt;
use std::time::Duration;

use md5::{Digest, Md5};
use serde::de::DeserializeOwned;
use url::Url;

use crate::error::SubsonicError;
use crate::models::*;

/// Subsonic protocol version NavAmp speaks.
const API_VERSION: &str = "1.16.1";
/// Client identifier sent as `c=` (shows up in Navidrome's client list).
const CLIENT_NAME: &str = "navamp";
/// Request timeout for every call, including `connect`.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Salt length in random bytes (hex-encoded on the wire, 24 chars).
const SALT_BYTES: usize = 12;

/// Async client for a Subsonic/OpenSubsonic server (Navidrome's native API).
///
/// Built via [`SubsonicClient::connect`], which performs a `ping` and returns
/// a configured client only on success. Authentication follows the spec's
/// salt+token scheme: `token = md5(password + salt)` with a random salt per
/// client. The password is never stored, and neither token nor salt ever
/// appears in `Debug` output or error messages.
///
/// This crate is a pure API client: no playback, caching, or sync logic —
/// callers drive the library walk (`get_artists` → `get_album`) themselves.
///
/// # Example
///
/// ```no_run
/// # async fn demo() -> Result<(), navamp_subsonic::SubsonicError> {
/// let client = navamp_subsonic::SubsonicClient::connect(
///     "https://music.example.org",
///     "admin",
///     "correct horse battery staple",
/// )
/// .await?;
///
/// for album_id in ["al-17"] {
///     let album = client.get_album(album_id).await?;
///     println!("{} — {} tracks", album.name, album.songs.len());
/// }
///
/// // Hand the URL to any HTTP-range-capable player; nothing is fetched here.
/// let url = client.stream_url(
///     album_id_placeholder(),
///     navamp_subsonic::StreamFormat::Mp3,
///     Some(256),
/// );
/// # Ok(())
/// # }
/// # fn album_id_placeholder() -> &'static str { "mf-42" }
/// ```
#[derive(Clone)]
pub struct SubsonicClient {
    http: reqwest::Client,
    /// `…/rest/` — normalized with a trailing slash so `join(endpoint)` appends.
    base: Url,
    username: String,
    token: String,
    salt: String,
    server_version: Option<String>,
}

// Never derive Debug here: it would leak the auth token and salt.
impl fmt::Debug for SubsonicClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SubsonicClient")
            .field("base", &self.base)
            .field("username", &self.username)
            .field("server_version", &self.server_version)
            .finish_non_exhaustive()
    }
}

impl SubsonicClient {
    /// Connects to `base_url` and verifies credentials with a `ping`.
    ///
    /// On success returns a configured client with the server version
    /// captured ([`SubsonicClient::server_version`]); otherwise
    /// [`SubsonicError::Auth`] for bad credentials, or whatever the transport
    /// produced.
    ///
    /// `base_url` is normalized by [`normalize_base_url`]: a missing scheme
    /// assumes `https://`, a missing `/rest` suffix is appended, trailing
    /// slashes and stray query/fragment are tolerated.
    pub async fn connect(
        base_url: &str,
        username: &str,
        password: &str,
    ) -> Result<Self, SubsonicError> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .gzip(true)
            .user_agent(concat!("navamp/", env!("CARGO_PKG_VERSION")))
            .build()?;

        let (salt, token) = credentials(password);
        let mut client = Self {
            http,
            base: normalize_base_url(base_url)?,
            username: username.to_owned(),
            token,
            salt,
            server_version: None,
        };

        let ping = client.ping().await?;
        client.server_version = Some(ping.server_version);
        Ok(client)
    }

    /// Server version reported during [`SubsonicClient::connect`].
    pub fn server_version(&self) -> Option<&str> {
        self.server_version.as_deref()
    }

    /// Verifies connectivity/auth; returns what the server says about itself.
    pub async fn ping(&self) -> Result<PingInfo, SubsonicError> {
        let inner = self.request("ping.view", &[]).await?;
        let resp: PingResponse = payload(inner)?;
        Ok(PingInfo {
            // OpenSubsonic servers report `serverVersion`; classic Subsonic
            // only has the protocol `version`. Prefer the former.
            server_version: resp.server_version.or(resp.version).unwrap_or_default(),
        })
    }

    /// All artists, flattened out of the `getArtists` index groups (A–Z, #).
    pub async fn get_artists(&self) -> Result<Vec<ArtistSummary>, SubsonicError> {
        let inner = self.request("getArtists.view", &[]).await?;
        let resp: GetArtistsResponse = payload(inner)?;
        Ok(resp
            .artists
            .index
            .into_iter()
            .flat_map(|g| g.artist)
            .collect())
    }

    /// One album with its songs inline — the leaf of the sync walk.
    pub async fn get_album(&self, id: &str) -> Result<AlbumDetail, SubsonicError> {
        let inner = self
            .request("getAlbum.view", &[("id", id.to_owned())])
            .await?;
        let resp: GetAlbumResponse = payload(inner)?;
        Ok(resp.album)
    }

    /// `search3`: songs + albums matching `query`.
    ///
    /// A limit of `0` is passed through to the server (Subsonic treats it as
    /// "no results" for that category); there is no pagination — callers
    /// wanting more should raise the limits.
    pub async fn search3(
        &self,
        query: &str,
        song_limit: u32,
        album_limit: u32,
        artist_limit: u32,
    ) -> Result<SearchResults, SubsonicError> {
        let inner = self
            .request(
                "search3.view",
                &[
                    ("query", query.to_owned()),
                    ("songCount", song_limit.to_string()),
                    ("albumCount", album_limit.to_string()),
                    ("artistCount", artist_limit.to_string()),
                ],
            )
            .await?;
        let resp: Search3Response = payload(inner)?;
        Ok(SearchResults {
            songs: resp.result.song,
            albums: resp.result.album,
        })
    }

    /// All playlists visible to the user.
    pub async fn get_playlists(&self) -> Result<Vec<PlaylistSummary>, SubsonicError> {
        let inner = self.request("getPlaylists.view", &[]).await?;
        let resp: GetPlaylistsResponse = payload(inner)?;
        Ok(resp.playlists.playlist)
    }

    /// One playlist's tracks, in playlist order.
    pub async fn get_playlist(&self, id: &str) -> Result<Vec<SongSummary>, SubsonicError> {
        let inner = self
            .request("getPlaylist.view", &[("id", id.to_owned())])
            .await?;
        let resp: GetPlaylistResponse = payload(inner)?;
        Ok(resp.playlist.entry)
    }

    /// Builds the streaming URL for a song. Pure builder — no request.
    ///
    /// [`StreamFormat::Original`] omits `format`/`maxBitRate` entirely so the
    /// server serves the stored file; the transcode formats add `format` and
    /// `maxBitRate` (when given) so Navidrome transcodes server-side.
    pub fn stream_url(&self, id: &str, format: StreamFormat, bitrate_kbps: Option<u32>) -> Url {
        let mut url = self.endpoint_url("stream.view");
        {
            let mut query = url.query_pairs_mut();
            for (key, value) in self.auth_params() {
                query.append_pair(key, &value);
            }
            query.append_pair("id", id);
            match format {
                StreamFormat::Original => {}
                StreamFormat::Mp3 | StreamFormat::Opus => {
                    query.append_pair(
                        "format",
                        match format {
                            StreamFormat::Mp3 => "mp3",
                            StreamFormat::Opus => "opus",
                            StreamFormat::Original => unreachable!(),
                        },
                    );
                    if let Some(bitrate) = bitrate_kbps {
                        query.append_pair("maxBitRate", &bitrate.to_string());
                    }
                }
            }
        }
        url
    }

    /// Builds the cover-art URL for an art id (song or album), optionally
    /// scaled to a square `size` (pixels). Pure builder — no request.
    pub fn cover_art_url(&self, id: &str, size: Option<u32>) -> Url {
        let mut url = self.endpoint_url("getCoverArt.view");
        {
            let mut query = url.query_pairs_mut();
            for (key, value) in self.auth_params() {
                query.append_pair(key, &value);
            }
            query.append_pair("id", id);
            if let Some(size) = size {
                query.append_pair("size", &size.to_string());
            }
        }
        url
    }

    /// Scrobbles a play: `submission = false` marks "now playing",
    /// `submission = true` marks the track as played (send near completion).
    /// `time_ms` backdates the event (spec: milliseconds since epoch).
    pub async fn scrobble(
        &self,
        id: &str,
        submission: bool,
        time_ms: Option<u64>,
    ) -> Result<(), SubsonicError> {
        let mut params = vec![
            ("id", id.to_owned()),
            ("submission", submission.to_string()),
        ];
        if let Some(time_ms) = time_ms {
            params.push(("time", time_ms.to_string()));
        }
        self.request("scrobble.view", &params).await?;
        Ok(())
    }

    /// Stars an item (song/album/artist id).
    pub async fn star(&self, id: &str) -> Result<(), SubsonicError> {
        self.request("star.view", &[("id", id.to_owned())]).await?;
        Ok(())
    }

    /// Removes a star from an item (song/album/artist id).
    pub async fn unstar(&self, id: &str) -> Result<(), SubsonicError> {
        self.request("unstar.view", &[("id", id.to_owned())])
            .await?;
        Ok(())
    }

    /// Sets the rating for an item, 0 (no rating) through 5. Values outside
    /// `0..=5` are rejected locally before any request is made.
    pub async fn rate(&self, id: &str, rating: u8) -> Result<(), SubsonicError> {
        if rating > 5 {
            return Err(SubsonicError::Server {
                code: 0,
                message: format!("invalid rating {rating}: must be 0..=5"),
            });
        }
        self.request(
            "setRating.view",
            &[("id", id.to_owned()), ("rating", rating.to_string())],
        )
        .await?;
        Ok(())
    }

    // -- internals ---------------------------------------------------------

    /// Absolute URL for an endpoint, auth-free (auth goes into the query).
    fn endpoint_url(&self, endpoint: &str) -> Url {
        // `base` is absolute and ends in '/', and endpoint names are static
        // relative paths, so join cannot fail.
        self.base
            .join(endpoint)
            .expect("static endpoint path is always a valid relative reference")
    }

    /// The six standard Subsonic query parameters, in spec order.
    fn auth_params(&self) -> Vec<(&'static str, String)> {
        vec![
            ("u", self.username.clone()),
            ("t", self.token.clone()),
            ("s", self.salt.clone()),
            ("v", API_VERSION.to_owned()),
            ("c", CLIENT_NAME.to_owned()),
            ("f", "json".to_owned()),
        ]
    }

    /// GET `endpoint`, check HTTP status, unwrap the `subsonic-response`
    /// envelope and map `status: "failed"` bodies to errors. Returns the
    /// envelope's inner object (everything except envelope-level fields).
    async fn request(
        &self,
        endpoint: &str,
        params: &[(&str, String)],
    ) -> Result<serde_json::Value, SubsonicError> {
        let mut url = self.endpoint_url(endpoint);
        {
            let mut query = url.query_pairs_mut();
            for (key, value) in self.auth_params() {
                query.append_pair(key, &value);
            }
            for (key, value) in params {
                query.append_pair(key, value);
            }
        }

        let response = self.http.get(url).send().await?;
        let status = response.status();
        if !status.is_success() {
            return Err(SubsonicError::UnexpectedStatus(status.as_u16()));
        }

        let body = response.text().await?;
        let envelope: Envelope = serde_json::from_str(&body)?;
        let inner = envelope.response;

        // A missing `status` is a malformed envelope; reuse serde to raise Json.
        #[derive(serde::Deserialize)]
        struct StatusOnly {
            status: String,
        }
        let head: StatusOnly = serde_json::from_value(inner.clone())?;

        match head.status.as_str() {
            "ok" => Ok(inner),
            _ => {
                let code = inner
                    .pointer("/error/code")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0);
                let message = inner
                    .pointer("/error/message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown subsonic error")
                    .to_owned();
                // 40 = wrong username/password, 41 = token auth unsupported
                // (e.g. LDAP users) — both mean "credentials won't work here".
                if code == 40 || code == 41 {
                    Err(SubsonicError::Auth)
                } else {
                    Err(SubsonicError::Server { code, message })
                }
            }
        }
    }
}

/// Normalizes a user-supplied Navidrome base URL into an absolute `…/rest/`
/// URL (trailing slash kept so [`Url::join`] appends endpoints).
///
/// Tolerated input shapes: `host`, `host:port`, `http(s)://host[:port][/path]`,
/// with or without a trailing `/rest` and/or trailing slash. A missing scheme
/// is assumed to be `https`. Any query/fragment on the input is dropped.
///
/// # Example
///
/// ```
/// # use navamp_subsonic::normalize_base_url;
/// let url = normalize_base_url("demo.navidrome.org").unwrap();
/// assert_eq!(url.as_str(), "https://demo.navidrome.org/rest/");
/// ```
pub fn normalize_base_url(base: &str) -> Result<Url, SubsonicError> {
    let trimmed = base.trim();
    if trimmed.is_empty() {
        return Err(SubsonicError::Url(url::ParseError::EmptyHost));
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    };
    let mut url = Url::parse(&with_scheme)?;

    let path = url.path().trim_end_matches('/');
    if path.is_empty() {
        url.set_path("/rest");
    } else if !path.ends_with("/rest") {
        url.set_path(&format!("{path}/rest"));
    }
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

/// Random salt + its matching `md5(password + salt)` token (hex).
fn credentials(password: &str) -> (String, String) {
    let mut bytes = [0u8; SALT_BYTES];
    getrandom::fill(&mut bytes).expect("OS randomness is available");
    let salt = hex(&bytes);
    let token = auth_token(password, &salt);
    (salt, token)
}

/// `token = md5(password + salt)`, lowercase hex — the Subsonic auth scheme.
fn auth_token(password: &str, salt: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(password.as_bytes());
    hasher.update(salt.as_bytes());
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Deserialize the endpoint payload out of the envelope's inner object.
fn payload<T: DeserializeOwned>(inner: serde_json::Value) -> Result<T, SubsonicError> {
    Ok(serde_json::from_value(inner)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_md5_of_password_plus_salt() {
        // Known vector: md5("sesame" + "c19b2d") = 26719a1196d2a940705a59634eb18eab
        assert_eq!(
            auth_token("sesame", "c19b2d"),
            "26719a1196d2a940705a59634eb18eab"
        );
    }

    #[test]
    fn salts_are_random_per_client() {
        let (salt_a, _) = credentials("pw");
        let (salt_b, _) = credentials("pw");
        assert_ne!(salt_a, salt_b);
        assert!(salt_a.len() >= 6, "spec wants salts of at least 6 chars");
    }
}
