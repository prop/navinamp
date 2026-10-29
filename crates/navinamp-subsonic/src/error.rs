//! Error type for the Subsonic client.

use thiserror::Error;

/// Everything that can go wrong while talking to a Subsonic/Navidrome server.
///
/// Error codes 40 and 41 (wrong username/password, and "token authentication
/// not supported for LDAP users") are both collapsed into [`SubsonicError::Auth`]
/// so callers can treat them as a single "re-enter your credentials" case.
#[derive(Debug, Error)]
pub enum SubsonicError {
    /// The server rejected the credentials (Subsonic error codes 40/41).
    #[error("authentication failed: wrong username or password")]
    Auth,

    /// The server answered HTTP 200 but the Subsonic envelope reported
    /// `"status": "failed"` with an error code/message pair.
    #[error("subsonic error {code}: {message}")]
    Server {
        /// Subsonic error code (see the spec's error code table).
        code: i64,
        /// Human-readable message from the server.
        message: String,
    },

    /// Transport-level failure (DNS, connect, timeout, TLS).
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    /// The configured base URL could not be parsed into a valid URL.
    #[error("invalid base URL: {0}")]
    Url(#[from] url::ParseError),

    /// The response body was not the expected JSON (or was missing a
    /// required field of the Subsonic envelope/payload).
    #[error("malformed JSON response: {0}")]
    Json(#[from] serde_json::Error),

    /// A non-2xx HTTP status (the Subsonic layer was never reached).
    #[error("unexpected HTTP status: {0}")]
    UnexpectedStatus(u16),
}
