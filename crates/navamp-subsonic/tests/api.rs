//! Integration tests against a local wiremock server — zero real network.
//!
//! Every fixture is Navidrome-shaped JSON (v1.16.1 envelope, `openSubsonic`
//! flag, `al-`/`ar-`/`mf-`/`pl-` id style).

use std::collections::HashMap;

use md5::{Digest, Md5};
use navamp_subsonic::{
    AlbumSummary, ArtistSummary, PingInfo, PlaylistSummary, StreamFormat, SubsonicClient,
    SubsonicError, normalize_base_url,
};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const USERNAME: &str = "admin";
const PASSWORD: &str = "sesame";

/// Wraps an endpoint payload in a realistic `subsonic-response` envelope.
fn ok_response(payload: Value) -> String {
    let mut envelope = json!({
        "status": "ok",
        "version": "1.16.1",
        "type": "navidrome",
        "serverVersion": "0.53.0",
        "openSubsonic": true,
    });
    if let (Some(fields), Some(extra)) = (envelope.as_object_mut(), payload.as_object()) {
        for (key, value) in extra {
            fields.insert(key.clone(), value.clone());
        }
    }
    Value::String(serde_json::to_string(&json!({ "subsonic-response": envelope })).unwrap())
        .as_str()
        .unwrap()
        .to_owned()
}

fn failed_response(code: i64, message: &str) -> String {
    serde_json::to_string(&json!({
        "subsonic-response": {
            "status": "failed",
            "version": "1.16.1",
            "error": { "code": code, "message": message }
        }
    }))
    .unwrap()
}

/// Mounts a canned JSON 200 response for one endpoint.
async fn mock_endpoint(server: &MockServer, endpoint: &str, body: String) {
    Mock::given(method("GET"))
        .and(path(format!("/rest/{endpoint}.view")))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(server)
        .await;
}

/// All requests the mock received for one endpoint, in order.
async fn requests_to(server: &MockServer, endpoint: &str) -> Vec<Request> {
    let suffix = format!("/rest/{endpoint}.view");
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request| request.url.path() == suffix)
        .collect()
}

/// Decoded query parameters of a captured request.
fn query(request: &Request) -> HashMap<String, String> {
    request
        .url
        .query()
        .unwrap_or_default()
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (key.to_owned(), value.to_owned())
        })
        .collect()
}

fn md5_hex(input: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

async fn connected_client(server: &MockServer) -> SubsonicClient {
    mock_endpoint(server, "ping", ok_response(json!({}))).await;
    SubsonicClient::connect(&server.uri(), USERNAME, PASSWORD)
        .await
        .expect("connect against mock")
}

// -- 1. auth ---------------------------------------------------------------

#[tokio::test]
async fn connect_sends_spec_compliant_auth_params() {
    let server = MockServer::start().await;
    let client = connected_client(&server).await;

    let pings = requests_to(&server, "ping").await;
    assert_eq!(pings.len(), 1, "connect performs exactly one ping");
    let params = query(&pings[0]);

    assert_eq!(params.get("u").map(String::as_str), Some("admin"));
    assert_eq!(params.get("v").map(String::as_str), Some("1.16.1"));
    assert_eq!(params.get("c").map(String::as_str), Some("navamp"));
    assert_eq!(params.get("f").map(String::as_str), Some("json"));

    let salt = params["s"].clone();
    assert!(
        salt.len() >= 6 && salt.bytes().all(|b| b.is_ascii_hexdigit()),
        "salt must be ≥6 hex chars, got {salt:?}"
    );
    // The core of the scheme: token = md5(password + salt), recomputed here.
    assert_eq!(params["t"], md5_hex(&format!("{PASSWORD}{salt}")));

    assert_eq!(client.server_version(), Some("0.53.0"));
}

// -- 2. ping ----------------------------------------------------------------

#[tokio::test]
async fn ping_parses_server_version() {
    let server = MockServer::start().await;
    let client = connected_client(&server).await;

    let info: PingInfo = client.ping().await.expect("ping ok");
    assert_eq!(info.server_version, "0.53.0");
    assert_eq!(requests_to(&server, "ping").await.len(), 2);
}

#[tokio::test]
async fn failed_body_code_40_maps_to_auth() {
    let server = MockServer::start().await;
    mock_endpoint(
        &server,
        "ping",
        failed_response(40, "Wrong username or password"),
    )
    .await;

    let error = SubsonicClient::connect(&server.uri(), USERNAME, "wrong")
        .await
        .expect_err("code 40 must fail connect");
    assert!(matches!(error, SubsonicError::Auth), "got: {error:?}");
}

#[tokio::test]
async fn failed_body_other_codes_map_to_server() {
    let server = MockServer::start().await;
    mock_endpoint(
        &server,
        "ping",
        failed_response(10, "Required parameter is missing."),
    )
    .await;

    let error = SubsonicClient::connect(&server.uri(), USERNAME, PASSWORD)
        .await
        .expect_err("status failed must fail connect");
    assert!(
        matches!(&error, SubsonicError::Server { code: 10, message }
            if message == "Required parameter is missing."),
        "got: {error:?}"
    );
}

// -- 3. library walk --------------------------------------------------------

#[tokio::test]
async fn get_artists_flattens_index_groups() {
    let server = MockServer::start().await;
    let client = connected_client(&server).await;
    mock_endpoint(
        &server,
        "getArtists",
        ok_response(json!({
            "artists": {
                "lastModified": 1690000000000i64,
                "ignoredArticles": "The El La Los Las Le Les",
                "index": [
                    { "name": "A", "artist": [
                        { "id": "ar-1", "name": "Abba", "albumCount": 8, "coverArt": "ar-1" },
                        { "id": "ar-2", "name": "Aphex Twin", "albumCount": 5 }
                    ]},
                    { "name": "#", "artist": [
                        { "id": "ar-3", "name": "13th Floor Elevators" }
                    ]}
                ]
            }
        })),
    )
    .await;

    let artists = client.get_artists().await.expect("artists parse");
    assert_eq!(
        artists,
        vec![
            ArtistSummary {
                id: "ar-1".into(),
                name: "Abba".into(),
                album_count: 8
            },
            ArtistSummary {
                id: "ar-2".into(),
                name: "Aphex Twin".into(),
                album_count: 5
            },
            ArtistSummary {
                id: "ar-3".into(),
                name: "13th Floor Elevators".into(),
                album_count: 0
            },
        ]
    );

    let requests = requests_to(&server, "getArtists").await;
    assert_eq!(
        query(&requests[0]).get("u").map(String::as_str),
        Some("admin")
    );
}

#[tokio::test]
async fn get_album_maps_songs_with_optional_fields_absent() {
    let server = MockServer::start().await;
    let client = connected_client(&server).await;
    mock_endpoint(
        &server,
        "getAlbum",
        ok_response(json!({
            "album": {
                "id": "al-1",
                "name": "Selected Ambient Works 85–92",
                "artist": "Aphex Twin",
                "artistId": "ar-2",
                "coverArt": "al-1",
                "songCount": 2,
                "duration": 839,
                "playCount": 4,
                "year": 1992,
                "genre": "Electronic",
                "created": "2001-02-03T04:05:06.000Z",
                "song": [
                    {
                        "id": "mf-1", "parent": "al-1", "isDir": false,
                        "title": "Xtal", "album": "Selected Ambient Works 85–92",
                        "artist": "Aphex Twin", "track": 1, "year": 1992,
                        "genre": "Electronic", "coverArt": "mf-1",
                        "size": 31615915, "contentType": "audio/flac", "suffix": "flac",
                        "duration": 293, "bitRate": 906, "disc": 1,
                        "playCount": 7, "albumId": "al-1", "artistId": "ar-2",
                        "path": "aphex/saw/01 Xtal.flac", "type": "music",
                        "created": "2001-02-03T04:05:06.000Z"
                    },
                    {
                        "id": "mf-2", "parent": "al-1", "isDir": false,
                        "title": "Tha", "album": "Selected Ambient Works 85–92",
                        "artist": "Aphex Twin", "track": 2,
                        "size": 61450214, "contentType": "audio/flac", "suffix": "flac",
                        "duration": 546, "bitRate": 905, "discNumber": 2,
                        "albumId": "al-1", "artistId": "ar-2", "type": "music"
                    }
                ]
            }
        })),
    )
    .await;

    let album = client.get_album("al-1").await.expect("album parses");
    assert_eq!(album.id, "al-1");
    assert_eq!(album.name, "Selected Ambient Works 85–92");
    assert_eq!(album.artist_id, "ar-2");
    assert_eq!(album.year, 1992);
    assert_eq!(album.genre.as_deref(), Some("Electronic"));
    assert_eq!(album.song_count, 2);
    assert_eq!(album.duration_secs, 839);
    assert_eq!(album.cover_art_id.as_deref(), Some("al-1"));
    assert_eq!(album.songs.len(), 2);

    // Fully-populated song (classic "disc" spelling).
    let xtal = &album.songs[0];
    assert_eq!((xtal.id.as_str(), xtal.title.as_str()), ("mf-1", "Xtal"));
    assert_eq!(xtal.parent_id, "al-1");
    assert_eq!(xtal.artist_id, "ar-2");
    assert_eq!(xtal.album_id, "al-1");
    assert_eq!(xtal.track_no, 1);
    assert_eq!(xtal.disc_no, 1);
    assert_eq!(xtal.year, 1992);
    assert_eq!(xtal.genre.as_deref(), Some("Electronic"));
    assert_eq!(xtal.duration_secs, 293);
    assert_eq!(xtal.bitrate_kbps, 906);
    assert_eq!(xtal.suffix, "flac");
    assert_eq!(xtal.content_type, "audio/flac");
    assert_eq!(xtal.cover_art_id.as_deref(), Some("mf-1"));
    assert_eq!(xtal.play_count, 7);
    assert!(!xtal.starred);

    // Sparse song: OpenSubsonic "discNumber" spelling, optionals absent.
    let tha = &album.songs[1];
    assert_eq!(tha.disc_no, 2);
    assert_eq!(tha.cover_art_id, None);
    assert_eq!(tha.genre, None);
    assert_eq!(tha.year, 0);
    assert_eq!(tha.play_count, 0);
    assert!(!tha.starred);

    let requests = requests_to(&server, "getAlbum").await;
    assert_eq!(
        query(&requests[0]).get("id").map(String::as_str),
        Some("al-1")
    );
}

// -- 4. search + playlists ----------------------------------------------------

#[tokio::test]
async fn search3_with_no_hits_returns_empty_results() {
    let server = MockServer::start().await;
    let client = connected_client(&server).await;
    mock_endpoint(
        &server,
        "search3",
        ok_response(json!({ "searchResult3": {} })),
    )
    .await;

    let results = client
        .search3("zzz", 20, 10, 10)
        .await
        .expect("empty search");
    assert_eq!(results.songs, vec![]);
    assert_eq!(results.albums, vec![]);

    let requests = requests_to(&server, "search3").await;
    let params = query(&requests[0]);
    assert_eq!(params.get("query").map(String::as_str), Some("zzz"));
    assert_eq!(params.get("songCount").map(String::as_str), Some("20"));
    assert_eq!(params.get("albumCount").map(String::as_str), Some("10"));
    assert_eq!(params.get("artistCount").map(String::as_str), Some("10"));
}

#[tokio::test]
async fn playlists_listed_and_entries_round_trip() {
    let server = MockServer::start().await;
    let client = connected_client(&server).await;
    mock_endpoint(
        &server,
        "getPlaylists",
        ok_response(json!({
            "playlists": { "playlist": [
                {
                    "id": "pl-1", "name": "Road trip", "comment": "",
                    "owner": "admin", "public": true, "songCount": 12,
                    "duration": 2707, "created": "2024-05-01T10:00:00.000Z",
                    "changed": "2024-05-01T10:00:00.000Z", "coverArt": "pl-1"
                },
                { "id": "pl-2", "name": "Quiet", "owner": "admin", "songCount": 1, "duration": 293 }
            ]}
        })),
    )
    .await;
    mock_endpoint(
        &server,
        "getPlaylist",
        ok_response(json!({
            "playlist": {
                "id": "pl-2", "name": "Quiet", "owner": "admin",
                "songCount": 1, "duration": 293, "public": false,
                "entry": [
                    {
                        "id": "mf-1", "parent": "al-1", "isDir": false,
                        "title": "Xtal", "album": "Selected Ambient Works 85–92",
                        "artist": "Aphex Twin", "track": 1, "year": 1992,
                        "contentType": "audio/flac", "suffix": "flac",
                        "duration": 293, "bitRate": 906,
                        "albumId": "al-1", "artistId": "ar-2", "type": "music"
                    }
                ]
            }
        })),
    )
    .await;

    let playlists = client.get_playlists().await.expect("playlists parse");
    assert_eq!(
        playlists,
        vec![
            PlaylistSummary {
                id: "pl-1".into(),
                name: "Road trip".into(),
                song_count: 12,
                duration_secs: 2707
            },
            PlaylistSummary {
                id: "pl-2".into(),
                name: "Quiet".into(),
                song_count: 1,
                duration_secs: 293
            },
        ]
    );

    let entries = client.get_playlist("pl-2").await.expect("entries parse");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, "mf-1");
    assert_eq!(entries[0].title, "Xtal");
    assert_eq!(entries[0].duration_secs, 293);

    let requests = requests_to(&server, "getPlaylist").await;
    assert_eq!(
        query(&requests[0]).get("id").map(String::as_str),
        Some("pl-2")
    );
}

#[tokio::test]
async fn search3_maps_hits() {
    let server = MockServer::start().await;
    let client = connected_client(&server).await;
    mock_endpoint(
        &server,
        "search3",
        ok_response(json!({
            "searchResult3": {
                "song": [
                    { "id": "mf-9", "parent": "al-1", "title": "On", "artist": "Aphex Twin",
                      "album": "On", "track": 1, "contentType": "audio/flac", "suffix": "flac",
                      "duration": 305, "bitRate": 983, "albumId": "al-1", "artistId": "ar-2" }
                ],
                "album": [
                    { "id": "al-7", "name": "On", "artist": "Aphex Twin", "artistId": "ar-2",
                      "coverArt": "al-7", "songCount": 4, "duration": 1420 }
                ],
                "artist": [ { "id": "ar-2", "name": "Aphex Twin", "albumCount": 5 } ]
            }
        })),
    )
    .await;

    let results = client.search3("on", 20, 10, 10).await.expect("search ok");
    assert_eq!(results.songs.len(), 1);
    assert_eq!(results.songs[0].title, "On");
    assert_eq!(
        results.albums,
        vec![AlbumSummary {
            id: "al-7".into(),
            name: "On".into(),
            artist: "Aphex Twin".into(),
            artist_id: "ar-2".into(),
            song_count: 4,
            duration_secs: 1420,
            cover_art_id: Some("al-7".into()),
        }]
    );
}

// -- 5. URL builders ----------------------------------------------------------

#[tokio::test]
async fn stream_url_original_has_no_transcode_params() {
    let server = MockServer::start().await;
    let client = connected_client(&server).await;

    let url = client.stream_url("mf-1", StreamFormat::Original, None);
    assert_eq!(
        url.path(),
        "/rest/stream.view",
        "path must target the stream endpoint"
    );
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let keys: Vec<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
    assert!(
        !keys.contains(&"format"),
        "original must not request transcode"
    );
    assert!(!keys.contains(&"maxBitRate"));
    assert!(keys.contains(&"id"));
    // auth rides along so the URL is directly playable
    assert!(keys.contains(&"u") && keys.contains(&"t") && keys.contains(&"s"));
    let id = pairs.iter().find(|(k, _)| k == "id").unwrap();
    assert_eq!(id.1, "mf-1");
}

#[tokio::test]
async fn stream_url_transcode_params() {
    let server = MockServer::start().await;
    let client = connected_client(&server).await;

    let mp3 = client.stream_url("mf-1", StreamFormat::Mp3, Some(256));
    let query: HashMap<String, String> = mp3
        .query_pairs()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    assert_eq!(query.get("format").map(String::as_str), Some("mp3"));
    assert_eq!(query.get("maxBitRate").map(String::as_str), Some("256"));
    assert_eq!(query.get("id").map(String::as_str), Some("mf-1"));

    let opus = client.stream_url("mf-2", StreamFormat::Opus, Some(128));
    let query: HashMap<String, String> = opus
        .query_pairs()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    assert_eq!(query.get("format").map(String::as_str), Some("opus"));
    assert_eq!(query.get("maxBitRate").map(String::as_str), Some("128"));

    // Transcode without an explicit bitrate: format only, server default.
    let bare = client.stream_url("mf-3", StreamFormat::Mp3, None);
    assert!(bare.query().unwrap().contains("format=mp3"));
    assert!(!bare.query().unwrap().contains("maxBitRate"));
}

#[tokio::test]
async fn cover_art_url_passes_id_and_optional_size() {
    let server = MockServer::start().await;
    let client = connected_client(&server).await;

    let sized = client.cover_art_url("al-1", Some(300));
    assert_eq!(sized.path(), "/rest/getCoverArt.view");
    let query: HashMap<String, String> = sized
        .query_pairs()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    assert_eq!(query.get("id").map(String::as_str), Some("al-1"));
    assert_eq!(query.get("size").map(String::as_str), Some("300"));
    assert!(query.contains_key("t") && query.contains_key("s"));

    let no_size = client.cover_art_url("al-1", None);
    assert!(!no_size.query().unwrap().contains("size"));
}

#[test]
fn normalize_base_url_tolerates_user_input_shapes() {
    let cases = [
        ("demo.navidrome.org", "https://demo.navidrome.org/rest/"),
        (
            "demo.navidrome.org:4533",
            "https://demo.navidrome.org:4533/rest/",
        ),
        ("http://localhost:4533", "http://localhost:4533/rest/"),
        ("http://localhost:4533/", "http://localhost:4533/rest/"),
        ("http://localhost:4533/rest", "http://localhost:4533/rest/"),
        ("http://localhost:4533/rest/", "http://localhost:4533/rest/"),
        (
            "http://localhost:4533/music",
            "http://localhost:4533/music/rest/",
        ),
        (
            "http://localhost:4533/music/",
            "http://localhost:4533/music/rest/",
        ),
        (
            "http://localhost:4533/music?x=1#frag",
            "http://localhost:4533/music/rest/",
        ),
    ];
    for (input, expected) in cases {
        let url = normalize_base_url(input).unwrap_or_else(|e| panic!("{input}: {e}"));
        assert_eq!(url.as_str(), expected, "input: {input}");
    }

    assert!(normalize_base_url("").is_err());
    assert!(normalize_base_url("https://").is_err());
}

#[tokio::test]
async fn connect_tolerates_base_url_variants_against_a_real_server() {
    let server = MockServer::start().await;
    mock_endpoint(&server, "ping", ok_response(json!({}))).await;

    for base in [
        server.uri(),                      // http://ip:port
        format!("{}/", server.uri()),      // trailing slash
        format!("{}/rest", server.uri()),  // rest already present
        format!("{}/rest/", server.uri()), // rest + slash
    ] {
        let client = SubsonicClient::connect(&base, USERNAME, PASSWORD)
            .await
            .unwrap_or_else(|e| panic!("base {base}: {e}"));
        assert_eq!(client.server_version(), Some("0.53.0"));
    }
    assert_eq!(requests_to(&server, "ping").await.len(), 4);
}

// -- 6. write endpoints ---------------------------------------------------------

#[tokio::test]
async fn scrobble_star_unstar_rate_send_correct_params() {
    let server = MockServer::start().await;
    let client = connected_client(&server).await;
    for endpoint in ["scrobble", "star", "unstar", "setRating"] {
        mock_endpoint(&server, endpoint, ok_response(json!({}))).await;
    }

    client
        .scrobble("mf-1", false, Some(1690000000123))
        .await
        .expect("now-playing scrobble");
    client
        .scrobble("mf-1", true, None)
        .await
        .expect("submission scrobble");
    client.star("mf-1").await.expect("star");
    client.unstar("mf-1").await.expect("unstar");
    client.rate("mf-1", 4).await.expect("rate");

    let scrobbles = requests_to(&server, "scrobble").await;
    assert_eq!(scrobbles.len(), 2);
    let now_playing = query(&scrobbles[0]);
    assert_eq!(now_playing.get("id").map(String::as_str), Some("mf-1"));
    assert_eq!(
        now_playing.get("submission").map(String::as_str),
        Some("false")
    );
    assert_eq!(
        now_playing.get("time").map(String::as_str),
        Some("1690000000123")
    );
    let submission = query(&scrobbles[1]);
    assert_eq!(
        submission.get("submission").map(String::as_str),
        Some("true")
    );
    assert!(!submission.contains_key("time"));

    let stars = requests_to(&server, "star").await;
    assert_eq!(query(&stars[0]).get("id").map(String::as_str), Some("mf-1"));
    let unstars = requests_to(&server, "unstar").await;
    assert_eq!(
        query(&unstars[0]).get("id").map(String::as_str),
        Some("mf-1")
    );

    let ratings = requests_to(&server, "setRating").await;
    assert_eq!(ratings.len(), 1);
    let rating = query(&ratings[0]);
    assert_eq!(rating.get("id").map(String::as_str), Some("mf-1"));
    assert_eq!(rating.get("rating").map(String::as_str), Some("4"));
}

#[tokio::test]
async fn out_of_range_rating_is_rejected_without_a_request() {
    let server = MockServer::start().await;
    let client = connected_client(&server).await;

    let error = client.rate("mf-1", 6).await.expect_err("6 is not a rating");
    assert!(
        matches!(&error, SubsonicError::Server { code: 0, message }
        if message.contains("0..=5")),
        "got: {error:?}"
    );
    assert!(
        requests_to(&server, "setRating").await.is_empty(),
        "no request may leave the client for an invalid rating"
    );
}

#[tokio::test]
async fn non_2xx_maps_to_unexpected_status() {
    let server = MockServer::start().await;
    mock_endpoint(&server, "ping", ok_response(json!({}))).await;
    let client = connected_client(&server).await;

    Mock::given(method("GET"))
        .and(path("/rest/star.view"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let error = client.star("mf-1").await.expect_err("503 must fail");
    assert!(
        matches!(error, SubsonicError::UnexpectedStatus(503)),
        "got: {error:?}"
    );
}

// -- 7. malformed responses ------------------------------------------------------

#[tokio::test]
async fn malformed_json_maps_to_json_error() {
    let server = MockServer::start().await;
    mock_endpoint(&server, "ping", ok_response(json!({}))).await;
    let client = connected_client(&server).await;

    Mock::given(method("GET"))
        .and(path("/rest/getArtists.view"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(r#"{"subsonic-res"#), // truncated JSON
        )
        .mount(&server)
        .await;

    let error = client.get_artists().await.expect_err("garbage must fail");
    assert!(matches!(error, SubsonicError::Json(_)), "got: {error:?}");
}

#[tokio::test]
async fn missing_envelope_maps_to_json_error() {
    let server = MockServer::start().await;
    mock_endpoint(&server, "ping", ok_response(json!({}))).await;
    let client = connected_client(&server).await;

    Mock::given(method("GET"))
        .and(path("/rest/getArtists.view"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"totally":"not subsonic"}"#))
        .mount(&server)
        .await;

    let error = client.get_artists().await.expect_err("must fail");
    assert!(matches!(error, SubsonicError::Json(_)), "got: {error:?}");
}

// -- the token must never leak through Debug ------------------------------------

#[tokio::test]
async fn debug_output_does_not_leak_token_or_salt() {
    let server = MockServer::start().await;
    let client = connected_client(&server).await;

    let debug = format!("{client:?}");
    let requests = requests_to(&server, "ping").await;
    let params = query(&requests[0]);
    let (token, salt) = (params["t"].clone(), params["s"].clone());

    assert!(!debug.contains(&token), "Debug leaked the token: {debug}");
    assert!(!debug.contains(&salt), "Debug leaked the salt: {debug}");
    assert!(
        !debug.contains(PASSWORD),
        "Debug leaked the password: {debug}"
    );
    assert!(
        debug.contains("username"),
        "Debug should stay useful: {debug}"
    );
}
