//! Typed, minimal response models for the endpoints NaVinaMP uses.
//!
//! All structs use `#[serde(rename_all = "camelCase")]` so they serialize
//! camelCase when they later cross Tauri IPC to the TypeScript UI, matching
//! the Subsonic JSON wire format on the way in. Fields Navidrome may omit
//! (`starred`, `coverArt`, `genre`, ...) are defaulted or `Option` — a
//! missing optional never fails a parse.

use serde::{Deserialize, Serialize};

/// Result of `ping`: what the server told us about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PingInfo {
    /// Server build version (`serverVersion`, falling back to the protocol
    /// `version` field on non-OpenSubsonic servers).
    pub server_version: String,
}

/// One artist from the `getArtists` index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistSummary {
    pub id: String,
    pub name: String,
    /// Number of albums, as reported by the server index.
    #[serde(default)]
    pub album_count: u32,
}

/// Minimal album shape used in search results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumSummary {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub artist_id: String,
    #[serde(default)]
    pub song_count: u32,
    #[serde(default, rename = "duration")]
    pub duration_secs: u64,
    #[serde(default, rename = "coverArt")]
    pub cover_art_id: Option<String>,
}

/// Full album from `getAlbum`, songs inline. This is the leaf of the
/// full-library sync walk (`getArtists` → `getAlbum` per album).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumDetail {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub artist_id: String,
    /// `0` when the server does not know the year.
    #[serde(default)]
    pub year: u32,
    #[serde(default)]
    pub genre: Option<String>,
    #[serde(default)]
    pub song_count: u32,
    #[serde(default, rename = "duration")]
    pub duration_secs: u64,
    #[serde(default, rename = "coverArt")]
    pub cover_art_id: Option<String>,
    /// The album's tracks. Deserialized from the `song` array (the spec
    /// name); `songs` is accepted as an alias for lenient servers.
    #[serde(default, rename = "song", alias = "songs")]
    pub songs: Vec<SongSummary>,
}

/// A playable track. Every field except `id`/`title` has a sane default so
/// partial server responses still parse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongSummary {
    pub id: String,
    #[serde(default, rename = "parent")]
    pub parent_id: String,
    pub title: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub artist_id: String,
    #[serde(default)]
    pub album: String,
    #[serde(default)]
    pub album_id: String,
    #[serde(default, rename = "track")]
    pub track_no: u32,
    #[serde(default, rename = "disc", alias = "discNumber")]
    pub disc_no: u32,
    /// `0` when the server does not know the year.
    #[serde(default)]
    pub year: u32,
    #[serde(default)]
    pub genre: Option<String>,
    #[serde(default, rename = "duration")]
    pub duration_secs: u64,
    #[serde(default, rename = "bitRate")]
    pub bitrate_kbps: u32,
    /// File extension, e.g. `"flac"` / `"mp3"`.
    #[serde(default)]
    pub suffix: String,
    /// MIME type, e.g. `"audio/flac"`.
    #[serde(default, rename = "contentType")]
    pub content_type: String,
    /// Sample rate in Hz, when the server reports it.
    #[serde(default, rename = "samplingRate")]
    pub sampling_rate: Option<u32>,
    /// Channel count, when the server reports it.
    #[serde(default, rename = "channelCount")]
    pub channel_count: Option<u32>,
    /// File size in bytes, when the server reports it.
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default, rename = "coverArt")]
    pub cover_art_id: Option<String>,
    #[serde(default)]
    pub play_count: u64,
    /// The user's 0–5 star rating (`userRating`); 0 when unrated or the
    /// server does not report ratings.
    #[serde(default, rename = "userRating")]
    pub user_rating: u8,
    /// Present-and-true only when starred on the server.
    #[serde(default)]
    pub starred: bool,
}

/// Result of `search3` (the parts NaVinaMP displays; artists come from
/// `getArtists` during sync instead).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SearchResults {
    pub songs: Vec<SongSummary>,
    pub albums: Vec<AlbumSummary>,
}

/// One playlist from `getPlaylists`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistSummary {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub song_count: u32,
    #[serde(default, rename = "duration")]
    pub duration_secs: u64,
}

/// Transcode target for `stream_url`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamFormat {
    /// Serve the original file: no `format`/`maxBitRate` parameters.
    Original,
    /// Transcode to MP3.
    Mp3,
    /// Transcode to Opus.
    Opus,
}

// ---------------------------------------------------------------------------
// Private wire structs: unwrap the per-endpoint envelope payloads.
// ---------------------------------------------------------------------------

/// `subsonic-response` wrapper; only the status/error part is read here.
#[derive(Debug, Deserialize)]
pub(crate) struct Envelope {
    #[serde(rename = "subsonic-response")]
    pub response: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PingResponse {
    #[serde(rename = "serverVersion", default)]
    pub server_version: Option<String>,
    /// Classic Subsonic field; fallback for non-OpenSubsonic servers.
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GetArtistsResponse {
    #[serde(default)]
    pub artists: Indexes,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GetArtistResponse {
    #[serde(default)]
    pub artist: ArtistWithAlbums,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ArtistWithAlbums {
    /// The artist's albums (`getArtist` payload; same shape as
    /// `getAlbumList2` entries).
    #[serde(default)]
    pub album: Vec<AlbumSummary>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Indexes {
    #[serde(default)]
    pub index: Vec<IndexGroup>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct IndexGroup {
    #[serde(default)]
    pub artist: Vec<ArtistSummary>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GetAlbumResponse {
    pub album: AlbumDetail,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Search3Response {
    #[serde(rename = "searchResult3", default)]
    pub result: SearchResult3,
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct SearchResult3 {
    #[serde(default)]
    pub song: Vec<SongSummary>,
    #[serde(default)]
    pub album: Vec<AlbumSummary>,
    // `artist` results arrive too, but NaVinaMP's search UI only wants
    // songs/albums; that field is ignored rather than carried around unread.
}

#[derive(Debug, Deserialize)]
pub(crate) struct GetPlaylistsResponse {
    #[serde(default)]
    pub playlists: Playlists,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreatePlaylistResponse {
    pub playlist: PlaylistSummary,
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct Playlists {
    #[serde(default)]
    pub playlist: Vec<PlaylistSummary>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GetPlaylistResponse {
    #[serde(default)]
    pub playlist: PlaylistEntries,
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct PlaylistEntries {
    #[serde(default)]
    pub entry: Vec<SongSummary>,
}
