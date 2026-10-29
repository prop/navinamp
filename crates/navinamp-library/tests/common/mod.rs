//! Shared test fixtures: hand-crafted, tiny, *valid* audio files written
//! in-test. No binary assets in the repo.
//!
//! - MP3: N repetitions of one MPEG-1 Layer III frame — 128 kbps, 44.1 kHz,
//!   stereo, no padding — i.e. `144 * 128000 / 44100 = 417.96 → 417` bytes
//!   of `FF FB 90 00` header + zero payload. Lofty walks frame headers for
//!   properties (duration = frames × 1152 / 44100), so the zero payload is
//!   decodable silence as far as any player is concerned. Tags: an ID3v2
//!   block written *through lofty* (`BoundTaggedFile`), exercising the same
//!   read path real files take.
//! - WAV: a canonical 44-byte RIFF header + PCM16 stereo data at 44.1 kHz,
//!   exact duration by construction.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{BoundTaggedFile, TaggedFileExt};
use lofty::tag::items::timestamp::Timestamp;
use lofty::tag::{Accessor, ItemKey, Tag, TagType};

/// The tag values a fixture carries; `String::new()` means "leave unset".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Meta {
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub genre: String,
    pub year: u16,
    pub track: u32,
    pub disc: u32,
}

/// One 417-byte MPEG-1 Layer III silence frame (128 kbps, 44.1 kHz, stereo).
fn mp3_frame() -> Vec<u8> {
    let mut frame = vec![0u8; 417];
    frame[0] = 0xFF; // sync
    frame[1] = 0xFB; // MPEG 1, Layer III, no CRC
    frame[2] = 0x90; // 128 kbps, 44100 Hz, no padding
    frame[3] = 0x00; // stereo, no emphasis
    frame
}

/// Raw MP3 audio data: `frames` silence frames.
pub fn mp3_bytes(frames: usize) -> Vec<u8> {
    let frame = mp3_frame();
    let mut out = Vec::with_capacity(frame.len() * frames);
    for _ in 0..frames {
        out.extend_from_slice(&frame);
    }
    out
}

/// Canonical 16-bit stereo 44.1 kHz WAV of `secs` seconds (silence).
/// (`#[allow]`: shared test module — not every test binary uses it.)
#[allow(dead_code)]
pub fn wav_bytes(secs: f64) -> Vec<u8> {
    let sample_rate: u32 = 44_100;
    let channels: u16 = 2;
    let bits_per_sample: u16 = 16;
    let block_align: u16 = channels * (bits_per_sample / 8); // 4 bytes/frame
    let byte_rate: u32 = sample_rate * u32::from(block_align);
    let data_len = (secs * f64::from(byte_rate)).round() as u32;

    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits_per_sample.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.resize(44 + data_len as usize, 0);
    out
}

/// Writes `bytes` to `path` and stamps `mtime` so scans see exactly the
/// change the test wants (mtime is the incremental-scan key, and file
/// systems truncate it unpredictably).
pub fn write_file(path: &Path, bytes: &[u8], mtime_secs: i64) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, bytes).unwrap();
    let stamp = std::time::SystemTime::UNIX_EPOCH
        + std::time::Duration::from_secs(mtime_secs.unsigned_abs());
    std::fs::File::open(path)
        .unwrap()
        .set_modified(stamp)
        .unwrap();
}

/// Attaches an ID3v2 tag (through lofty) to an existing MP3 fixture,
/// preserving its mtime.
pub fn tag_mp3(path: &Path, meta: &Meta) {
    let mtime = std::fs::metadata(path).unwrap().modified().unwrap();
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    let mut bound = BoundTaggedFile::read_from(file, ParseOptions::new()).unwrap();

    let mut tag = Tag::new(TagType::Id3v2);
    if !meta.title.is_empty() {
        tag.set_title(meta.title.clone());
    }
    if !meta.artist.is_empty() {
        tag.set_artist(meta.artist.clone());
    }
    if !meta.album.is_empty() {
        tag.set_album(meta.album.clone());
    }
    if !meta.genre.is_empty() {
        tag.set_genre(meta.genre.clone());
    }
    if !meta.album_artist.is_empty() {
        tag.insert_text(ItemKey::AlbumArtist, meta.album_artist.clone());
    }
    if meta.year > 0 {
        tag.set_date(Timestamp {
            year: meta.year,
            ..Timestamp::default()
        });
    }
    if meta.track > 0 {
        tag.set_track(meta.track);
    }
    if meta.disc > 0 {
        tag.set_disk(meta.disc);
    }
    bound.insert_tag(tag);
    bound.save(WriteOptions::new()).unwrap();

    // lofty rewrote the file; put the caller's mtime back.
    std::fs::File::open(path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
}

/// Convenience: MP3 fixture with tags, in one call.
pub fn write_mp3(path: &Path, frames: usize, meta: &Meta, mtime_secs: i64) {
    write_file(path, &mp3_bytes(frames), mtime_secs);
    if *meta != Meta::default() {
        tag_mp3(path, meta);
    }
}

/// A unique per-test scratch directory under the system temp dir, with
/// best-effort cleanup on drop (following the navinamp-skin test pattern —
/// no extra dev-dependencies).
pub struct Scratch {
    pub dir: PathBuf,
}

impl Scratch {
    pub fn new(label: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "navinamp-library-{label}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    /// Library DB path inside the scratch dir. (`#[allow]`: shared test
    /// module — not every test binary uses every helper.)
    #[allow(dead_code)]
    pub fn db(&self) -> PathBuf {
        self.dir.join("library.db")
    }

    #[allow(dead_code)]
    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
