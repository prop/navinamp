//! NavAmp's Subsonic/OpenSubsonic (Navidrome) API client.
//!
//! A small async [`SubsonicClient`] over `reqwest` (rustls TLS, 30 s
//! timeout) speaking Subsonic API v1.16.1 with salt+token authentication.
//! Endpoints return typed, serde-serializable models (`camelCase`, ready to
//! cross Tauri IPC); [`SubsonicClient::stream_url`] and
//! [`SubsonicClient::cover_art_url`] build URLs without fetching. There is
//! deliberately **no** playback, cache, or library-sync logic here — the
//! caller (the library/sync layer) drives the `get_artists` → `get_album`
//! walk and hands stream URLs to the audio engine.
//!
//! # Endpoints covered
//!
//! | Method | Subsonic endpoint |
//! |---|---|
//! | [`SubsonicClient::connect`] / [`SubsonicClient::ping`] | `ping` |
//! | [`SubsonicClient::get_artists`] | `getArtists` |
//! | [`SubsonicClient::get_artist`] | `getArtist` (artist → their albums) |
//! | [`SubsonicClient::get_album`] | `getAlbum` (songs inline) |
//! | [`SubsonicClient::search3`] | `search3` |
//! | [`SubsonicClient::get_playlists`] | `getPlaylists` |
//! | [`SubsonicClient::get_playlist`] | `getPlaylist` |
//! | [`SubsonicClient::stream_url`] | `stream` (URL only) |
//! | [`SubsonicClient::cover_art_url`] | `getCoverArt` (URL only) |
//! | [`SubsonicClient::scrobble`] | `scrobble` |
//! | [`SubsonicClient::star`] / [`SubsonicClient::unstar`] | `star` / `unstar` |
//! | [`SubsonicClient::rate`] | `setRating` |
//!
//! # Errors
//!
//! All fallible calls return [`SubsonicError`]. Subsonic's "HTTP 200 but
//! `status: failed`" bodies surface as [`SubsonicError::Server`]`{code, message}`,
//! except error codes 40/41 (bad credentials / token auth unsupported),
//! which collapse to [`SubsonicError::Auth`].

pub mod client;
pub mod error;
pub mod models;

pub use client::{SubsonicClient, normalize_base_url};
pub use error::SubsonicError;
pub use models::{
    AlbumDetail, AlbumSummary, ArtistSummary, PingInfo, PlaylistSummary, SearchResults,
    SongSummary, StreamFormat,
};
