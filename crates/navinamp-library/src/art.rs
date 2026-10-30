//! Album-art extraction and the content-addressed art cache.
//!
//! Policy, per audio file, evaluated inside the scanner's tag-read pass:
//!
//! 1. **Embedded art wins**: the tag's first `CoverFront` picture (falling
//!    back to its first picture of any type) is that file's art.
//! 2. **Folder art fills**: files without embedded art fall back to a
//!    sibling directory image — `folder.jpg|jpeg|png`, `cover.*`,
//!    `front.*`, case-insensitive, in that priority order — resolved at
//!    most once per directory per scan (memoized).
//! 3. **Bytes are stored once per unique content**: the art id is
//!    `hex(sha256(content))[..16]` and the payload lands at
//!    `<art_dir>/<id>.<sniffed ext>` — the extension comes from magic
//!    bytes, not the source's claim, so identical content always maps to
//!    the identical file and writes are skipped when it already exists.
//!    Ten tracks sharing one cover cost one cache file.
//!
//! `<art_dir>` is `<db dir>/art-cache` ([`CACHE_DIR`]), created on demand.
//! `Library::open` derives it from the database path; `:memory:`
//! databases have none and art extraction is disabled for them.
//!
//! **Incrementality**: unchanged files keep their `art_id` — art is derived
//! in the same pass that (re)reads tags, so unchanged files are never
//! opened. One exception: the *first* art-aware scan of a database
//! (`meta.art_backfilled` absent, handled in `scanner.rs`) re-derives art
//! for unchanged rows, so libraries indexed by pre-art builds gain covers
//! without a forced full rescan.
//!
//! **Garbage collection is deliberately absent**: cache files are
//! content-addressed and rows carry no refcounts, so track removals never
//! delete art. Orphans are harmless (dedup keeps the set small); sweeping
//! them is a later maintenance feature, not a correctness need.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use lofty::picture::{Picture, PictureType};
use sha2::{Digest, Sha256};

/// Name of the art-cache directory — a sibling of the library database
/// file. Re-exported by `navinamp-library` so the Tauri shell's
/// `navinamp-art` scheme handler resolves the exact same path.
pub(crate) const CACHE_DIR: &str = "art-cache";

/// Folder-image candidates (lower-cased file names), in priority order.
const FOLDER_ART_FILES: &[&str] = &[
    "folder.jpg",
    "folder.jpeg",
    "folder.png",
    "cover.jpg",
    "cover.jpeg",
    "cover.png",
    "front.jpg",
    "front.jpeg",
    "front.png",
];

/// Picks a file's embedded picture: the first `CoverFront` picture, or the
/// tag's first picture of any type when no front cover is tagged.
pub(crate) fn embedded_picture(pictures: &[Picture]) -> Option<&Picture> {
    pictures
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures.first())
}

/// Art id for `bytes`: the first 8 bytes of SHA-256, lower-case hex
/// (16 chars). Truncation is fine — ids key a cache, they don't guard
/// anything cryptographic.
pub(crate) fn content_id(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex(&digest[..8])
}

/// Sniffs the image format from magic bytes → canonical extension. Only
/// formats a webview can render are recognized; anything else (TIFF, …)
/// is skipped rather than cached undecodable.
pub(crate) fn sniff_ext(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else if bytes.starts_with(b"BM") {
        Some("bmp")
    } else if bytes.len() >= 12
        && bytes.get(..4) == Some(b"RIFF")
        && bytes.get(8..12) == Some(b"WEBP")
    {
        Some("webp")
    } else {
        None
    }
}

/// Writes `bytes` into the cache (once) and returns its art id.
///
/// `None` — never an error — when there is no cache dir (`:memory:`
/// databases), the bytes are not a recognized image, or the write fails:
/// a missing cover must never fail the file's track upsert.
pub(crate) fn store(art_dir: Option<&Path>, bytes: &[u8]) -> Option<String> {
    let dir = art_dir?;
    let ext = sniff_ext(bytes)?;
    let id = content_id(bytes);
    let path = dir.join(format!("{id}.{ext}"));
    if !path.exists() {
        fs::create_dir_all(dir).ok()?;
        // Content-addressed: losing a race with another writer would only
        // write the same bytes again, so a plain write is safe.
        fs::write(&path, bytes).ok()?;
    }
    Some(id)
}

/// Resolves the folder image for `dir`, memoized per scan: the first
/// readable candidate in [`FOLDER_ART_FILES`] order, stored under its
/// content id. `None` = this directory has no usable folder image.
pub(crate) fn folder_art(
    dir: &Path,
    art_dir: Option<&Path>,
    memo: &mut HashMap<PathBuf, Option<String>>,
) -> Option<String> {
    if let Some(hit) = memo.get(dir) {
        return hit.clone();
    }
    let resolved = resolve_folder_art(dir, art_dir);
    memo.insert(dir.to_owned(), resolved.clone());
    resolved
}

fn resolve_folder_art(dir: &Path, art_dir: Option<&Path>) -> Option<String> {
    // One read_dir, lower-cased file names → paths; then pick by priority.
    let mut candidates: HashMap<String, PathBuf> = HashMap::new();
    for entry in fs::read_dir(dir).ok()?.flatten() {
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        if let Some(name) = entry.file_name().to_str() {
            candidates.insert(name.to_ascii_lowercase(), entry.path());
        }
    }
    FOLDER_ART_FILES
        .iter()
        .filter_map(|name| candidates.get(*name))
        .filter_map(|path| fs::read(path).ok())
        // A present-but-unusable candidate (unreadable, unrecognized
        // bytes) falls through to the next name.
        .find_map(|bytes| store(art_dir, &bytes))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal but structurally valid PNG (1×1, transparent).
    fn png() -> Vec<u8> {
        vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x62, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ]
    }

    #[test]
    fn magic_byte_sniffing() {
        assert_eq!(sniff_ext(&png()), Some("png"));
        assert_eq!(sniff_ext(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
        assert_eq!(sniff_ext(b"GIF89a...."), Some("gif"));
        assert_eq!(sniff_ext(b"BM\x00\x00"), Some("bmp"));
        assert_eq!(sniff_ext(b"RIFF\x00\x00\x00\x00WEBP...."), Some("webp"));
        assert_eq!(sniff_ext(b"II*\x00 TIFF"), None);
        assert_eq!(sniff_ext(b""), None);
    }

    #[test]
    fn content_ids_are_16_hex_chars_and_content_keyed() {
        let a = content_id(b"same bytes");
        let b = content_id(b"same bytes");
        let c = content_id(b"other bytes");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 16);
        assert!(a.bytes().all(|byte| byte.is_ascii_hexdigit()));
        // Pinned against accidental hash/encoding changes.
        assert_eq!(
            a,
            hex(&Sha256::digest(b"same bytes")[..8]),
            "id must stay sha256[..8] hex"
        );
    }

    #[test]
    fn store_writes_once_and_skips_existing() {
        let dir =
            std::env::temp_dir().join(format!("navinamp-library-art-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let id = store(Some(&dir), &png()).unwrap();
        let file = dir.join(format!("{id}.png"));
        assert_eq!(std::fs::read(&file).unwrap(), png());

        // Second store of identical bytes: no new file, same id.
        assert_eq!(store(Some(&dir), &png()), Some(id));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);

        // Unsniffable bytes and a missing cache dir both degrade to None.
        assert_eq!(store(Some(&dir), b"not an image"), None);
        assert_eq!(store(None, &png()), None);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
