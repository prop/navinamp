# navinamp-subsonic

Async Subsonic/OpenSubsonic API v1.16.1 client for [NaVinaMP](../../README.md),
built for Navidrome (its native protocol). Pure API client: **no playback,
caching, or sync logic** — the library/sync layer drives the
`get_artists` → `get_album` walk and hands stream URLs to the audio engine.

## Usage

```rust,no_run
# async fn demo() -> Result<(), navinamp_subsonic::SubsonicError> {
let client = navinamp_subsonic::SubsonicClient::connect(
    "https://music.example.org", // scheme optional, /rest optional, slash optional
    "admin",
    "hunter2",
)
.await?;

let artists = client.get_artists().await?;           // flattened from index groups
let album = client.get_album("al-17").await?;         // songs inline
let hits = client.search3("aphex", 20, 10, 10).await?;
let playlists = client.get_playlists().await?;
let tracks = client.get_playlist("pl-1").await?;

// URL builders — nothing is fetched, hand them to any HTTP client:
let stream = client.stream_url("mf-42", navinamp_subsonic::StreamFormat::Mp3, Some(256));
let art = client.cover_art_url("al-17", Some(300));

client.scrobble("mf-42", true, None).await?;          // submission
client.scrobble("mf-42", false, Some(1690000000123)).await?; // now-playing
client.star("mf-42").await?;
client.rate("mf-42", 4).await?;
# Ok(()) }
```

## Authentication

Follows the Subsonic salt+token scheme (never sends the password):

- a random 24-hex-char salt is generated per client (`getrandom`),
- `token = md5(password + salt)` (`md-5`),
- every request carries `u, t=<token>, s=<salt>, v=1.16.1, c=navinamp, f=json`.

`SubsonicClient::connect` performs a `ping` and only returns a client on
success, capturing the server version (`server_version()`). Neither the
token, the salt, nor the password ever appears in `Debug` output or errors
(there is a regression test asserting exactly that). Note: token auth means
the password must be the real password — an API-key-style token will not
work; that is a Subsonic spec property, not a client choice.

## Endpoint coverage

| Method | Endpoint | Notes |
|---|---|---|
| `connect` / `ping` | `ping` | connect = ping + capture `serverVersion` |
| `get_artists` | `getArtists` | flattens `index[].artist[]` |
| `get_album` | `getAlbum` | songs inline; leaf of the sync walk |
| `search3` | `search3` | songs + albums (artist hits ignored by design) |
| `get_playlists` | `getPlaylists` | |
| `get_playlist` | `getPlaylist` | returns the `entry` tracks |
| `stream_url` | `stream` | **URL builder only**; `Original` sends no `format`/`maxBitRate`, `Mp3`/`Opus` add both (bitrate optional) |
| `cover_art_url` | `getCoverArt` | **URL builder only**; optional square `size` |
| `scrobble` | `scrobble` | `submission` flag + optional `time` (ms) |
| `star` / `unstar` | `star` / `unstar` | |
| `rate` | `setRating` | 0–5, rejected locally if out of range |

All models are `serde` camelCase both ways, so responses round-trip across
Tauri IPC to the TypeScript UI unchanged. Optional server fields
(`coverArt`, `genre`, `starred`, `playCount`, …) default instead of failing
the parse.

## Errors (`SubsonicError`)

| Variant | Meaning |
|---|---|
| `Auth` | Subsonic codes 40/41 (bad credentials / token auth unsupported for LDAP) — including from `connect` |
| `Server { code, message }` | HTTP 200 with `"status": "failed"` |
| `Http(reqwest::Error)` | transport (DNS/timeout/TLS) |
| `Url(url::ParseError)` | unparseable base URL |
| `Json(serde_json::Error)` | malformed body / missing envelope or payload field |
| `UnexpectedStatus(u16)` | non-2xx HTTP status |

Base URLs are normalized by `normalize_base_url` (public, for preflight
validation in settings UIs): missing scheme → `https`, missing `/rest` →
appended, trailing slash added, stray query/fragment dropped.

## Testing

`cargo test -p navinamp-subsonic` — 2 unit + 20 integration tests + 2
doctests, all against a local `wiremock` server. Zero network access. The
auth test re-computes `md5(password + salt)` itself and compares it to the
`t=` param that actually left the client.

## Limitations (for the sync agent)

- **No pagination**: `get_artists` walks all index groups in one response
  (fine for Navidrome; `musicFolder` selection is not exposed).
  `search3` limits are caller-chosen; no `offset` paging.
- **OpenSubsonic extensions not modeled**: `openSubsonic` flag,
  `artistImageUrl`, explicit content-type headers on `stream` responses,
  etc. are ignored (serde skips unknown fields, so they remain
  forward-compatible).
- `star`/`unstar`/`rate` accept song ids; albumId/artistId work too per
  spec but are not separately typed.
- `stream_url` embeds auth in the query — URLs are bearer credentials;
  don't log them.
