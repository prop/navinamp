//! Playlist URL streams: validation, display metadata, and optional YouTube
//! resolution through a user-provided `yt-dlp`.
//!
//! Direct `http(s)` audio URLs need no backend help to play (the audio engine
//! already streams them); this module classifies them and derives the title /
//! artist shown in the playlist. YouTube links are not media URLs, so they are
//! resolved with `yt-dlp --dump-single-json`. Direct googlevideo URLs expire,
//! so the queue keeps the canonical video link and calls [`streams_resolve`]
//! again on every play.
//!
//! The helper is opt-in: it is detected on `PATH` (plus the usual Homebrew /
//! local-install directories, since a Finder-launched app has a tiny `PATH`)
//! or configured by the user in the Add URL dialog. It is never bundled.
//!
//! Security: the helper is invoked without a shell, `--` precedes the user
//! URL, output is captured with a timeout, and surfaced errors are scrubbed of
//! URLs (googlevideo links carry tokens).

use std::collections::HashMap;
use std::env;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::navidrome::scrub_urls;

/// Optional local override file (app config dir).
const CONFIG_FILE: &str = "streams.json";
/// Environment override for advanced users / portable installs.
const YTDLP_ENV: &str = "NAVINAMP_YTDLP";
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(60);
/// Resolved media URLs live in the queue; keep their headers long enough for
/// a prefetched window to reach playback, without growing forever.
const HEADERS_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const HEADERS_MAX: usize = 64;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct StreamConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    ytdlp_path: Option<String>,
}

/// Current state of the YouTube helper, shown in the Add URL dialog.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YtdlpStatus {
    /// The user-configured path (null = auto-detect).
    pub configured_path: Option<String>,
    /// The executable that will actually run (null = not found).
    pub resolved_path: Option<String>,
    pub found: bool,
    /// First line of `yt-dlp --version`, when available.
    pub version: Option<String>,
}

/// A validated playlist URL plus the display metadata derived from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedStreamUrl {
    /// Normalized URL stored in the queue.
    pub url: String,
    /// True for a single YouTube video link (requires yt-dlp).
    pub youtube: bool,
    /// Display title for direct streams; a placeholder for YouTube until the
    /// resolver returns the real title.
    pub title: String,
    /// Display artist (the host for direct streams, "YouTube" for video links).
    pub artist: String,
}

/// One YouTube resolution: display metadata plus the expiring direct media URL.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedStream {
    pub stream_url: String,
    pub title: String,
    pub artist: String,
    pub duration_secs: Option<f64>,
}

/// Cached entry: (`stored_at`, request headers).
type HeaderEntry = (Instant, Vec<(String, String)>);

/// Request headers for resolved media URLs, keyed by the exact URL handed to
/// the audio engine. Tauri commands run on the same process, so `audio_play`
/// can look them up after `streams_resolve` returns.
#[derive(Default)]
pub struct StreamHeaders {
    inner: Mutex<HashMap<String, HeaderEntry>>,
}

impl StreamHeaders {
    pub(crate) fn insert(&self, url: String, headers: Vec<(String, String)>) {
        if headers.is_empty() {
            return;
        }
        let mut map = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        map.retain(|_, (at, _)| at.elapsed() < HEADERS_TTL);
        if map.len() >= HEADERS_MAX
            && let Some(oldest) = map
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(url, _)| url.clone())
        {
            map.remove(&oldest);
        }
        map.insert(url, (Instant::now(), headers));
    }

    pub(crate) fn get(&self, url: &str) -> Vec<(String, String)> {
        let mut map = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        map.retain(|_, (at, _)| at.elapsed() < HEADERS_TTL);
        map.get(url)
            .map(|(_, headers)| headers.clone())
            .unwrap_or_default()
    }
}

#[tauri::command]
pub fn streams_parse_url(url: String) -> Result<ParsedStreamUrl, String> {
    classify_stream_url(&url)
}

#[tauri::command]
pub async fn streams_ytdlp_status(app: AppHandle) -> Result<YtdlpStatus, String> {
    let config = load_config(&app);
    tauri::async_runtime::spawn_blocking(move || status_for(config))
        .await
        .map_err(|e| format!("yt-dlp status task failed: {e}"))
}

/// Save a custom `yt-dlp` path (`None` clears it and returns to auto-detect).
#[tauri::command]
pub async fn streams_set_ytdlp_path(
    app: AppHandle,
    path: Option<String>,
) -> Result<YtdlpStatus, String> {
    tauri::async_runtime::spawn_blocking(move || set_ytdlp_path(&app, path))
        .await
        .map_err(|e| format!("yt-dlp config task failed: {e}"))?
}

fn set_ytdlp_path(app: &AppHandle, path: Option<String>) -> Result<YtdlpStatus, String> {
    let clean = path.map(|p| p.trim().to_owned()).filter(|p| !p.is_empty());
    if let Some(raw) = clean.as_deref() {
        let resolved =
            resolve_binary(raw).ok_or_else(|| format!("`{raw}` is not an executable file"))?;
        if ytdlp_version(&resolved).is_none() {
            return Err(format!(
                "`{}` did not answer `--version` — is it yt-dlp?",
                resolved.display()
            ));
        }
    }
    let mut config = load_config(app);
    config.ytdlp_path = clean;
    save_config(app, &config)?;
    Ok(status_for(config))
}

/// Resolve a YouTube video link to a playable direct audio URL + metadata.
#[tauri::command]
pub async fn streams_resolve(
    app: AppHandle,
    headers: State<'_, StreamHeaders>,
    url: String,
) -> Result<ResolvedStream, String> {
    let config = load_config(&app);
    let (resolved, request_headers) =
        tauri::async_runtime::spawn_blocking(move || resolve_stream(&config, &url))
            .await
            .map_err(|e| format!("stream resolver task failed: {e}"))??;
    headers.insert(resolved.stream_url.clone(), request_headers);
    Ok(resolved)
}

// ---------------------------------------------------------------------------
// URL classification
// ---------------------------------------------------------------------------

/// Validate a playlist URL and derive display metadata.
pub(crate) fn classify_stream_url(raw: &str) -> Result<ParsedStreamUrl, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("enter a URL first".into());
    }
    let url = reqwest::Url::parse(trimmed).map_err(|_| "that is not a valid URL".to_string())?;
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(format!("only http(s) links are supported (got `{other}:`)"));
        }
    }
    let host = url
        .host_str()
        .ok_or_else(|| "the URL has no host".to_string())?;
    let display_host = host.trim_start_matches("www.").to_owned();
    let youtube = youtube_video_id(&url).is_some();
    if !youtube && is_youtube_host(host) {
        return Err(
            "YouTube playlists, channels and home pages are not supported — paste a single video link"
                .into(),
        );
    }
    let title = if youtube {
        "YouTube video".to_owned()
    } else {
        url.path_segments()
            .and_then(|mut segments| segments.rfind(|s| !s.is_empty()))
            .map(decode_component)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| display_host.clone())
    };
    Ok(ParsedStreamUrl {
        url: url.to_string(),
        youtube,
        title,
        artist: if youtube {
            "YouTube".to_owned()
        } else {
            display_host
        },
    })
}

fn is_youtube_host(host: &str) -> bool {
    let host = host.trim_start_matches("www.").to_ascii_lowercase();
    matches!(
        host.as_str(),
        "youtube.com" | "m.youtube.com" | "music.youtube.com" | "youtu.be" | "youtube-nocookie.com"
    )
}

/// The video id for a supported single-video YouTube link shape.
///
/// Supported: `youtu.be/<id>`, `/watch?v=<id>`, `/shorts/<id>`,
/// `/embed/<id>`, `/live/<id>`. Playlists/channels return `None` so the
/// caller can distinguish them from a generic URL.
fn youtube_video_id(url: &reqwest::Url) -> Option<String> {
    let host = url
        .host_str()?
        .trim_start_matches("www.")
        .to_ascii_lowercase();
    let path = url.path().trim_end_matches('/');
    match host.as_str() {
        "youtu.be" => path
            .trim_start_matches('/')
            .split('/')
            .next()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        "youtube.com" | "m.youtube.com" | "music.youtube.com" | "youtube-nocookie.com" => {
            if path == "/watch" {
                url.query_pairs()
                    .find(|(key, _)| key == "v")
                    .map(|(_, value)| value.trim().to_owned())
                    .filter(|s| !s.is_empty())
            } else {
                ["/shorts/", "/embed/", "/live/"].iter().find_map(|prefix| {
                    path.strip_prefix(prefix)
                        .and_then(|rest| rest.split('/').next())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                })
            }
        }
        _ => None,
    }
}

/// Percent-decode one path segment (display title only; never used as a path).
fn decode_component(segment: &str) -> String {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2]))
        {
            out.push(hi * 16 + lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// yt-dlp discovery / configuration
// ---------------------------------------------------------------------------

fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("no config dir: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create `{}`: {e}", dir.display()))?;
    Ok(dir.join(CONFIG_FILE))
}

fn load_config(app: &AppHandle) -> StreamConfig {
    config_path(app)
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|raw| serde_json::from_str::<StreamConfig>(&raw).ok())
        .unwrap_or_default()
}

fn save_config(app: &AppHandle, config: &StreamConfig) -> Result<(), String> {
    let path = config_path(app)?;
    let json = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("cannot write `{}`: {e}", path.display()))
}

fn status_for(config: StreamConfig) -> YtdlpStatus {
    let configured_path = config.ytdlp_path.clone();
    let resolved = resolve_configured(&config);
    let resolved_path = resolved.as_ref().map(|p| p.display().to_string());
    let version = resolved.as_deref().and_then(ytdlp_version);
    YtdlpStatus {
        configured_path,
        found: resolved.is_some(),
        resolved_path,
        version,
    }
}

/// Resolve the helper to run: configured path > env override > PATH > common
/// install locations. A configured path is authoritative (no silent fallback).
fn resolve_configured(config: &StreamConfig) -> Option<PathBuf> {
    if let Some(raw) = config.ytdlp_path.as_deref() {
        return resolve_binary(raw);
    }
    if let Some(raw) = env::var_os(YTDLP_ENV)
        && let Some(path) = resolve_binary(&raw.to_string_lossy())
    {
        return Some(path);
    }
    find_on_path("yt-dlp").or_else(common_paths)
}

/// A configured value may be a full path (with `~`), a bare command name, or
/// a relative path.
fn resolve_binary(raw: &str) -> Option<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let path = expand_tilde(raw);
    if path.is_absolute() || path.components().count() > 1 {
        return is_executable(&path).then_some(path);
    }
    find_on_path(raw)
}

fn expand_tilde(raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/")
        && let Some(home) = home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(raw)
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let names: Vec<String> = if cfg!(windows) && !name.to_ascii_lowercase().ends_with(".exe") {
        vec![format!("{name}.exe"), name.to_owned()]
    } else {
        vec![name.to_owned()]
    };
    let paths = env::var_os("PATH")?;
    for dir in env::split_paths(&paths) {
        for name in &names {
            let candidate = dir.join(name);
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn common_paths() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(home) = home_dir() {
        candidates.push(home.join(".local/bin/yt-dlp"));
        candidates.push(home.join("bin/yt-dlp"));
    }
    #[cfg(target_os = "macos")]
    {
        candidates.push(PathBuf::from("/opt/homebrew/bin/yt-dlp"));
        candidates.push(PathBuf::from("/usr/local/bin/yt-dlp"));
    }
    #[cfg(target_os = "linux")]
    {
        candidates.push(PathBuf::from("/usr/local/bin/yt-dlp"));
        candidates.push(PathBuf::from("/usr/bin/yt-dlp"));
    }
    #[cfg(windows)]
    {
        if let Some(local) = env::var_os("LOCALAPPDATA") {
            candidates.push(PathBuf::from(local).join("Programs/yt-dlp/yt-dlp.exe"));
            candidates.push(PathBuf::from(local).join("yt-dlp/yt-dlp.exe"));
        }
    }
    candidates.into_iter().find(|path| is_executable(path))
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn ytdlp_version(path: &Path) -> Option<String> {
    let output = run_process(path, &["--version"], VERSION_TIMEOUT).ok()?;
    if !output.success {
        return None;
    }
    output
        .stdout
        .lines()
        .next()
        .map(|line| line.trim().to_owned())
        .filter(|line| !line.is_empty())
}

// ---------------------------------------------------------------------------
// resolution
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct YtDlpInfo {
    title: Option<String>,
    fulltitle: Option<String>,
    uploader: Option<String>,
    channel: Option<String>,
    artist: Option<String>,
    creator: Option<String>,
    duration: Option<f64>,
    url: Option<String>,
    requested_downloads: Option<Vec<YtDlpDownload>>,
    http_headers: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
struct YtDlpDownload {
    url: Option<String>,
}

fn resolve_stream(
    config: &StreamConfig,
    raw_url: &str,
) -> Result<(ResolvedStream, Vec<(String, String)>), String> {
    let parsed = classify_stream_url(raw_url)?;
    if !parsed.youtube {
        return Err("only YouTube links need the stream resolver".into());
    }
    let binary = resolve_configured(config).ok_or_else(|| {
        "YouTube support needs yt-dlp. Install it (e.g. `brew install yt-dlp`) or set its path in ADD URL."
            .to_owned()
    })?;
    let output = run_process(
        &binary,
        &[
            "--ignore-config",
            "--no-playlist",
            "--no-warnings",
            "--no-progress",
            "--dump-single-json",
            "-f",
            "bestaudio/best",
            "--",
            parsed.url.as_str(),
        ],
        RESOLVE_TIMEOUT,
    )
    .map_err(|e| format!("could not run yt-dlp: {e}"))?;
    if !output.success {
        let detail = output
            .stderr
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("yt-dlp failed");
        return Err(scrub_urls(&format!(
            "yt-dlp could not resolve this link (update it if YouTube changed): {detail}"
        )));
    }
    parse_ytdlp_json(&output.stdout)
}

fn parse_ytdlp_json(stdout: &str) -> Result<(ResolvedStream, Vec<(String, String)>), String> {
    let start = stdout
        .find('{')
        .ok_or_else(|| "yt-dlp returned no video information".to_string())?;
    // Parse the first JSON value and ignore anything after it (a stray
    // warning line must not discard a valid result).
    let mut deserializer = serde_json::Deserializer::from_str(&stdout[start..]);
    let info = YtDlpInfo::deserialize(&mut deserializer)
        .map_err(|e| format!("could not parse yt-dlp output: {e}"))?;
    let stream_url = info
        .url
        .or_else(|| {
            info.requested_downloads
                .as_ref()
                .and_then(|downloads| downloads.iter().find_map(|d| d.url.clone()))
        })
        .ok_or_else(|| "yt-dlp returned no audio stream URL".to_string())?;
    if !stream_url.starts_with("http://") && !stream_url.starts_with("https://") {
        return Err("yt-dlp returned a non-http stream URL".into());
    }
    let title = clean(info.title)
        .or_else(|| clean(info.fulltitle))
        .unwrap_or_else(|| "YouTube video".to_owned());
    let artist = clean(info.artist)
        .or_else(|| clean(info.uploader))
        .or_else(|| clean(info.channel))
        .or_else(|| clean(info.creator))
        .unwrap_or_else(|| "YouTube".to_owned());
    let duration_secs = info
        .duration
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0);
    // yt-dlp knows which headers the CDN expects (UA/Referer/Origin/...).
    // Drop framing/hop-by-hop headers; reqwest owns those.
    let mut headers: Vec<(String, String)> = info
        .http_headers
        .unwrap_or_default()
        .into_iter()
        .filter(|(name, value)| !is_skipped_header(name) && !value.trim().is_empty())
        .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
        .collect();
    headers.sort_by(|a, b| a.0.cmp(&b.0));
    Ok((
        ResolvedStream {
            stream_url,
            title,
            artist,
            duration_secs,
        },
        headers,
    ))
}

fn is_skipped_header(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "host"
            | "content-length"
            | "connection"
            | "range"
            | "accept-encoding"
            | "transfer-encoding"
            | "proxy-connection"
            | "upgrade"
    )
}

fn clean(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

struct ProcessOutput {
    success: bool,
    stdout: String,
    stderr: String,
}

/// Run a helper with captured output and a hard deadline.
fn run_process(binary: &Path, args: &[&str], timeout: Duration) -> Result<ProcessOutput, String> {
    let mut child = Command::new(binary)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run `{}`: {e}", binary.display()))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "helper stdout unavailable".to_string())?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| "helper stderr unavailable".to_string())?;
    let stdout_thread = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let stderr_thread = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        bytes
    });

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("cannot wait for `{}`: {e}", binary.display()));
            }
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("`{}` timed out", binary.display()));
        }
        std::thread::sleep(Duration::from_millis(25));
    };

    let stdout = stdout_thread.join().unwrap_or_default();
    let stderr = stderr_thread.join().unwrap_or_default();
    Ok(ProcessOutput {
        success: status.success(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_video_shapes_are_youtube() {
        for raw in [
            "https://youtu.be/abc123",
            "https://www.youtube.com/watch?v=abc123",
            "https://music.youtube.com/watch?v=abc123",
            "https://youtube.com/shorts/abc123",
            "https://youtube.com/embed/abc123",
            "https://youtube.com/live/abc123",
            "https://m.youtube.com/watch?v=abc123&list=PL123",
        ] {
            let parsed = classify_stream_url(raw).expect(raw);
            assert!(parsed.youtube, "{raw}");
            assert_eq!(parsed.artist, "YouTube");
        }
    }

    #[test]
    fn youtube_playlists_and_channels_are_rejected() {
        for raw in [
            "https://www.youtube.com/playlist?list=PL123",
            "https://www.youtube.com/@somechannel",
            "https://www.youtube.com/",
            "https://youtu.be/",
        ] {
            let error = classify_stream_url(raw).expect_err(raw);
            assert!(error.contains("single video"), "{raw}: {error}");
        }
    }

    #[test]
    fn generic_streams_get_host_and_last_segment() {
        let parsed = classify_stream_url("https://radio.example.com/live/stream%20one.mp3?token=x")
            .expect("stream");
        assert!(!parsed.youtube);
        assert_eq!(parsed.artist, "radio.example.com");
        assert_eq!(parsed.title, "stream one.mp3");
        assert_eq!(
            parsed.url,
            "https://radio.example.com/live/stream%20one.mp3?token=x"
        );
    }

    #[test]
    fn host_fallback_title_and_www_trim() {
        let parsed = classify_stream_url("https://www.example.com/").expect("stream");
        assert_eq!(parsed.artist, "example.com");
        assert_eq!(parsed.title, "example.com");
    }

    #[test]
    fn invalid_schemes_and_inputs_are_rejected() {
        for raw in ["", "not a url", "file:///tmp/a.mp3", "javascript:alert(1)"] {
            assert!(classify_stream_url(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn parses_ytdlp_metadata() {
        let json = r#"{
            "title": "Song title",
            "uploader": "Some uploader",
            "duration": 123.5,
            "url": "https://media.example/audio"
        }"#;
        let (resolved, headers) = parse_ytdlp_json(json).expect("resolved");
        assert_eq!(resolved.stream_url, "https://media.example/audio");
        assert_eq!(resolved.title, "Song title");
        assert_eq!(resolved.artist, "Some uploader");
        assert_eq!(resolved.duration_secs, Some(123.5));
        assert!(headers.is_empty());
    }

    #[test]
    fn resolver_headers_are_filtered_and_sorted() {
        let json = r#"{
            "title": "Song",
            "url": "https://media.example/audio",
            "http_headers": {
                "User-Agent": "Test UA",
                "Referer": "https://example.com/",
                "Host": "ignored",
                "Range": "ignored"
            }
        }"#;
        let (_, headers) = parse_ytdlp_json(json).expect("resolved");
        assert_eq!(
            headers,
            vec![
                ("Referer".to_owned(), "https://example.com/".to_owned()),
                ("User-Agent".to_owned(), "Test UA".to_owned()),
            ]
        );
    }

    #[test]
    fn requested_downloads_is_a_stream_url_fallback() {
        let json = r#"{
            "title": "Song title",
            "channel": "Channel name",
            "requested_downloads": [{"url": "https://media.example/fallback"}]
        }"#;
        let (resolved, _) = parse_ytdlp_json(json).expect("resolved");
        assert_eq!(resolved.stream_url, "https://media.example/fallback");
        assert_eq!(resolved.artist, "Channel name");
        assert_eq!(resolved.duration_secs, None);
    }

    #[test]
    fn non_http_media_urls_are_rejected() {
        let json = r#"{"title":"x","url":"file:///tmp/a.mp3"}"#;
        assert!(parse_ytdlp_json(json).is_err());
    }

    #[test]
    fn missing_helper_error_is_actionable() {
        let config = StreamConfig {
            ytdlp_path: Some("/definitely/not/yt-dlp".into()),
        };
        let error = resolve_stream(&config, "https://youtu.be/abc123").unwrap_err();
        assert!(error.contains("needs yt-dlp"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn resolves_through_a_configured_helper() {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt as _;

        let path = std::env::temp_dir().join(format!(
            "navinamp-fake-ytdlp-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let mut script = std::fs::File::create(&path).expect("create fake helper");
        writeln!(script, "#!/bin/sh").unwrap();
        writeln!(script, "cat <<'EOF'").unwrap();
        let json = r#"{"title":"Fake song","uploader":"Fake artist","duration":42.5,"url":"https://media.example/audio","http_headers":{"User-Agent":"Test UA","Host":"ignored"}}"#;
        writeln!(script, "{json}").unwrap();
        writeln!(script, "EOF").unwrap();
        drop(script);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake helper");

        let config = StreamConfig {
            ytdlp_path: Some(path.display().to_string()),
        };
        let result = resolve_stream(&config, "https://youtu.be/abc123");
        let _ = std::fs::remove_file(&path);
        let (resolved, headers) = result.expect("resolve through helper");
        assert_eq!(resolved.title, "Fake song");
        assert_eq!(resolved.artist, "Fake artist");
        assert_eq!(resolved.duration_secs, Some(42.5));
        assert_eq!(resolved.stream_url, "https://media.example/audio");
        assert_eq!(
            headers,
            vec![("User-Agent".to_owned(), "Test UA".to_owned())]
        );
    }
}
