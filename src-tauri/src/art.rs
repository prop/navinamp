//! The `navinamp-art` URI scheme: serves cached album-art bytes straight
//! into the webview for `<img src>` use — no base64 IPC payloads, no
//! per-image commands. Registered in `lib.rs` on the app builder.
//!
//! URL shapes handled (all of them — the webview normalizes custom scheme
//! URLs differently per platform, and hand-built URLs may use the naive
//! form):
//!
//! - `navinamp-art://<art_id>` — naive form (the id lands in the host slot)
//! - `navinamp-art://localhost/<art_id>` — macOS/Linux custom protocol form
//! - `http://navinamp-art.localhost/<art_id>` — Windows/Android form
//!
//! An art id must be exactly 16 ASCII hex chars (our sha256[..16-bit]
//! content ids), which makes the handler path-traversal-proof by
//! construction. Cache files are immutable per id (content-addressed), so
//! hits carry a one-year `Cache-Control: immutable`; misses are uncached
//! 404s (a later scan may still produce that id).

use std::path::Path;

use tauri::http::{Response, Uri};
use tauri::{Runtime, UriSchemeContext};

use crate::library::art_dir;

/// The registered scheme name.
pub const SCHEME: &str = "navinamp-art";

/// (extension, MIME) pairs probed in this order when serving an id. Only
/// formats the scanner caches (see `navinamp-library`'s `art::sniff_ext`).
const ART_TYPES: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("bmp", "image/bmp"),
    ("webp", "image/webp"),
];

/// Protocol entry point (see `lib.rs` for the registration).
pub fn handle<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: tauri::http::Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    let dir = art_dir(ctx.app_handle()).ok();
    serve(dir.as_deref(), request.uri())
}

/// Serves `uri` out of `art_dir` (`None` → 404: no app data dir).
fn serve(art_dir: Option<&Path>, uri: &Uri) -> Response<Vec<u8>> {
    let Some(dir) = art_dir else {
        return not_found();
    };
    let Some(id) = art_id_from_uri(uri) else {
        return not_found();
    };
    for (ext, mime) in ART_TYPES {
        match std::fs::read(dir.join(format!("{id}.{ext}"))) {
            Ok(bytes) => {
                return Response::builder()
                    .status(200)
                    .header("Content-Type", *mime)
                    // Content-addressed: an id never maps to different
                    // bytes, so the webview may cache it forever.
                    .header("Cache-Control", "public, max-age=31536000, immutable")
                    .body(bytes)
                    .expect("static response parts are always valid");
            }
            Err(_) => continue,
        }
    }
    not_found()
}

/// Extracts the art id from a request URI (see the module docs for the
/// accepted shapes). Anything that is not exactly 16 hex chars is `None`.
fn art_id_from_uri(uri: &Uri) -> Option<&str> {
    let raw: &str = match uri.host() {
        // `navinamp-art://localhost/<id>` and `http://navinamp-art.localhost/<id>`
        Some(host)
            if host.eq_ignore_ascii_case("localhost")
                || host.to_ascii_lowercase().ends_with(".localhost") =>
        {
            uri.path()
        }
        // `navinamp-art://<id>` — the whole id is parsed as the host.
        Some(host) => host,
        None => uri.path(),
    };
    let id = raw
        .trim_start_matches('/')
        .split(['?', '#'])
        .next()
        .unwrap_or("");
    is_art_id(id).then_some(id)
}

fn is_art_id(id: &str) -> bool {
    id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

fn not_found() -> Response<Vec<u8>> {
    Response::builder()
        .status(404)
        .header("Content-Type", "text/plain; charset=utf-8")
        // Misses stay cheap to retry — a scan may land this id later.
        .header("Cache-Control", "no-store")
        .body(Vec::new())
        .expect("static response parts are always valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(s: &str) -> Uri {
        s.parse().expect("test URIs are well-formed")
    }

    #[test]
    fn ids_extract_from_every_url_shape() {
        let id = "0123456789abcdef";
        for raw in [
            format!("navinamp-art://{id}"),
            format!("navinamp-art://localhost/{id}"),
            format!("navinamp-art://localhost/{id}?v=2"),
            format!("http://navinamp-art.localhost/{id}"),
            format!("http://NAVINAMP-ART.localhost/{id}"), // host case varies
        ] {
            assert_eq!(art_id_from_uri(&uri(&raw)), Some(id), "uri: {raw}");
        }
        // Upper-case hex is still hex; the cache writes lower-case but
        // file systems on macOS/Windows resolve it either way.
        assert_eq!(
            art_id_from_uri(&uri("navinamp-art://localhost/0123456789ABCDEF")),
            Some("0123456789ABCDEF")
        );
    }

    #[test]
    fn anything_else_is_rejected() {
        for raw in [
            "navinamp-art://localhost/../../../etc/passwd",
            "navinamp-art://localhost/xyz",               // not hex
            "navinamp-art://localhost/0123456789abcde",   // 15 chars
            "navinamp-art://localhost/0123456789abcdef0", // 17 chars
            "navinamp-art://localhost/",
            "navinamp-art://localhost",
        ] {
            assert_eq!(art_id_from_uri(&uri(raw)), None, "uri: {raw}");
        }
    }

    #[test]
    fn serving_reads_cache_files_with_mime_and_cache_headers() {
        let dir = std::env::temp_dir().join(format!("navinamp-art-serve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let id = "0011223344556677";
        let jpg_id = "ff99887766554433";
        std::fs::write(dir.join(format!("{id}.png")), b"png-bytes").unwrap();
        std::fs::write(dir.join(format!("{jpg_id}.jpg")), b"jpg-bytes").unwrap();
        // decoys that must never be picked up
        std::fs::write(dir.join("not-an-id.png"), b"x").unwrap();
        std::fs::create_dir_all(dir.join(format!("{id}.txt"))).unwrap();

        let hit = serve(Some(&dir), &uri(&format!("navinamp-art://localhost/{id}")));
        assert_eq!(hit.status(), 200);
        assert_eq!(hit.headers()["content-type"], "image/png");
        assert_eq!(
            hit.headers()["cache-control"],
            "public, max-age=31536000, immutable"
        );
        assert_eq!(hit.body(), b"png-bytes");

        let jpg = serve(Some(&dir), &uri(&format!("navinamp-art://{jpg_id}")));
        assert_eq!(jpg.status(), 200);
        assert_eq!(jpg.headers()["content-type"], "image/jpeg");
        assert_eq!(jpg.body(), b"jpg-bytes");

        let miss = serve(
            Some(&dir),
            &uri("navinamp-art://localhost/9999999999999999"),
        );
        assert_eq!(miss.status(), 404);
        assert_eq!(miss.headers()["cache-control"], "no-store");
        assert!(miss.body().is_empty());

        // No cache dir at all: still a clean 404.
        assert_eq!(
            serve(None, &uri("navinamp-art://localhost/9999999999999999")).status(),
            404
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
