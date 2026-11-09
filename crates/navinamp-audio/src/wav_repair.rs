//! Tolerant RIFF/WAVE repair for files whose header lengths symphonia's
//! strict reader rejects ("wav: missing data chunk").
//!
//! Real-world WAVs sometimes carry a RIFF size of 0 (streaming writers), an
//! under-reported RIFF length, or a zero `data` chunk length — every media
//! player tolerates these, but symphonia's chunk walker stops at the header
//! value and reports the file has no data chunk at all. This module walks
//! the chunk table tolerantly (clamping bogus lengths to the file size),
//! locates the real `fmt ` + `data` chunks, and produces the corrected
//! header fields. [`PatchedSource`] then serves the original file with those
//! few length bytes rewritten, so symphonia probes and decodes it unchanged.
//!
//! Good files get no patches at all: the wrapper is skipped entirely and
//! behavior is byte-identical to probing the plain file.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use symphonia::core::io::MediaSource;

/// Byte offsets + corrected little-endian values for the two header length
/// fields the strict reader depends on.
#[derive(Debug, Clone)]
pub struct WavRepair {
    /// `(file offset, corrected little-endian u32)` pairs, sorted by offset.
    pub patches: Vec<(u64, [u8; 4])>,
}

/// Walk the RIFF chunk table of a WAVE file tolerantly. Returns the repairs
/// needed for symphonia's strict reader, or `None` when the file is not a
/// RIFF/WAVE, has no decodable `data` chunk (bogus chunk table), or needs
/// no patches at all.
pub fn scan(file: &mut File) -> Option<WavRepair> {
    let file_len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(0)).ok()?;

    let mut head = [0u8; 12];
    file.read_exact(&mut head).ok()?;
    if &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return None;
    }

    let mut patches: Vec<(u64, [u8; 4])> = Vec::new();
    let mut pos = 12u64;
    let mut fmt_seen = false;
    let mut data: Option<(u64, u64)> = None;

    // Walk chunk headers, clamping every length to what the file actually
    // holds — a bogus length (0, under-run, garbage) must not end the scan
    // or swallow the data chunk.
    let mut guard = 0u32;
    while pos + 8 <= file_len {
        guard += 1;
        if guard > 1_000_000 {
            return None; // pathological: refuse to spin
        }
        if file.seek(SeekFrom::Start(pos)).is_err() {
            break;
        }
        let mut hdr = [0u8; 8];
        if file.read_exact(&mut hdr).is_err() {
            break;
        }
        let declared = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as u64;
        let avail = file_len.saturating_sub(pos + 8);
        let len = declared.min(avail);

        match &hdr[0..4] {
            b"fmt " => fmt_seen = true,
            b"data" => {
                // Data is *normally* the last chunk: everything after its
                // header is audio. A declared length of 0 (streaming
                // writers) is clamped to exactly the remaining bytes, which
                // truncates any trailing metadata chunks — the right
                // trade-off for a decode-only repair.
                let effective = if len == 0 { avail } else { len };
                data = Some((pos + 8, effective));
                break;
            }
            _ => {}
        }
        // RIFF chunks are word-aligned: odd lengths carry a pad byte.
        pos = pos + 8 + len + (len & 1);
    }

    let (data_start, data_len) = data?;
    if !fmt_seen || data_len == 0 {
        return None; // nothing symphonia could decode even with fixed lengths
    }

    // Correct RIFF size: bytes after the 8-byte RIFF header, through the
    // end of the data chunk. RIFF lengths are u32; a cluster that cannot be
    // described by u32 stays broken (the strict error is kept rather than
    // silently decoding a truncated prefix).
    let riff_len_pos = 4u64;
    let correct_riff = u32::try_from((data_start + data_len).saturating_sub(8))
        .ok()?
        .to_le_bytes();
    let mut head = [0u8; 12];
    file.seek(SeekFrom::Start(0)).ok()?;
    file.read_exact(&mut head).ok()?;
    if head[4..8] != correct_riff {
        patches.push((riff_len_pos, correct_riff));
    }

    // Correct data chunk length (same u32 limit).
    let data_len_pos = data_start.saturating_sub(4);
    let correct_data = u32::try_from(data_len).ok()?.to_le_bytes();
    file.seek(SeekFrom::Start(data_len_pos)).ok()?;
    let mut have = [0u8; 4];
    file.read_exact(&mut have).ok()?;
    if have != correct_data {
        patches.push((data_len_pos, correct_data));
    }

    if patches.is_empty() {
        return None;
    }
    Some(WavRepair { patches })
}

/// A [`MediaSource`] serving the original file with a few header length
/// bytes rewritten. Everything else is a passthrough; the virtual cursor
/// equals the underlying file cursor because patches never change length.
pub struct PatchedSource {
    inner: File,
    patches: Vec<(u64, [u8; 4])>,
    pos: u64,
}

impl PatchedSource {
    pub fn new(mut inner: File, mut patches: Vec<(u64, [u8; 4])>) -> Self {
        // Patches never change the length, so the virtual cursor is the
        // underlying file cursor; start both at zero. Sorting keeps the
        // in-read patch application order-independent.
        let _ = inner.seek(SeekFrom::Start(0));
        patches.sort_by_key(|(off, _)| *off);
        Self {
            inner,
            patches,
            pos: 0,
        }
    }
}

impl Read for PatchedSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        let end = self.pos + n as u64;
        for &(at, bytes) in &self.patches {
            if at >= end {
                continue; // patch starts beyond this read
            }
            for (i, byte) in bytes.iter().enumerate() {
                let abs = at + i as u64;
                if abs >= self.pos && abs < end {
                    buf[(abs - self.pos) as usize] = *byte;
                }
            }
        }
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for PatchedSource {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let len = self.inner.metadata()?.len();
        let target = match pos {
            SeekFrom::Start(p) => p as i64,
            SeekFrom::End(d) => len as i64 + d,
            SeekFrom::Current(d) => self.pos as i64 + d,
        };
        let target = target.max(0) as u64;
        self.inner.seek(SeekFrom::Start(target))?;
        self.pos = target;
        Ok(target)
    }
}

impl MediaSource for PatchedSource {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        self.inner.metadata().ok().map(|m| m.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn wav_bytes(samples: usize) -> Vec<u8> {
        let data_len = (samples * 2) as u32;
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data_len).to_le_bytes());
        v.extend_from_slice(b"WAVE");
        v.extend_from_slice(b"fmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&8000u32.to_le_bytes());
        v.extend_from_slice(&16000u32.to_le_bytes());
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data_len.to_le_bytes());
        for s in std::iter::repeat_n(0i16, samples) {
            v.extend_from_slice(&s.to_le_bytes());
        }
        v
    }

    fn temp_wav(name: &str, bytes: &[u8]) -> (File, std::path::PathBuf) {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "navinamp-wavrepair-{}-{name}.wav",
            std::process::id()
        ));
        let mut f = File::create(&path).expect("create fixture");
        f.write_all(bytes).expect("write fixture");
        let f = File::open(&path).expect("reopen fixture");
        (f, path)
    }

    #[test]
    fn scan_rejects_non_riff_and_healthy_files() {
        let (mut file, path) = temp_wav("healthy.wav", &wav_bytes(64));
        assert!(scan(&mut file).is_none(), "healthy wav needs no repair");
        drop(file);
        cleanup(path);

        let mut path2 = std::env::temp_dir();
        path2.push(format!(
            "navinamp-wavrepair-{}-notriff.wav",
            std::process::id()
        ));
        std::fs::write(&path2, b"not a riff file at all").expect("write");
        let mut f = File::open(&path2).expect("open");
        assert!(scan(&mut f).is_none(), "non-RIFF is not scanned");
        drop(f);
        std::fs::remove_file(&path2).ok();
    }

    #[test]
    fn scan_repairs_zero_riff_len() {
        let mut bytes = wav_bytes(64);
        bytes[4..8].copy_from_slice(&0u32.to_le_bytes());
        let (mut file, path) = temp_wav("zero-riff.wav", &bytes);
        let repair = scan(&mut file).expect("repair needed");
        assert_eq!(repair.patches.len(), 1);
        cleanup(path);
    }

    #[test]
    fn scan_repairs_under_reported_riff_len() {
        // Non-zero but too small: the RIFF size header stops well before
        // the data chunk ends. Same code path as the zero case.
        let mut bytes = wav_bytes(64);
        bytes[4..8].copy_from_slice(&20u32.to_le_bytes());
        let (mut file, path) = temp_wav("short-riff.wav", &bytes);
        let repair = scan(&mut file).expect("repair needed");
        let (at, value) = repair.patches[0];
        assert_eq!(at, 4, "patches the RIFF chunk length");
        assert_eq!(value, 164u32.to_le_bytes());
        cleanup(path);
    }

    #[test]
    fn scan_repairs_zero_data_len() {
        let mut bytes = wav_bytes(64);
        bytes[40..44].copy_from_slice(&0u32.to_le_bytes());
        let (mut file, path) = temp_wav("zero-data.wav", &bytes);
        let repair = scan(&mut file).expect("repair needed");
        // Only the data chunk length needs fixing: the RIFF size header
        // was already consistent with the real content.
        assert_eq!(repair.patches.len(), 1);
        let (at, value) = repair.patches[0];
        assert_eq!(at, 40, "patches the data chunk length");
        assert_eq!(value, 128u32.to_le_bytes());
        cleanup(path);
    }

    #[test]
    fn patched_source_serves_corrected_bytes() {
        let mut bytes = wav_bytes(64);
        // Both header lengths broken: RIFF size 0 and data size 0.
        bytes[4..8].copy_from_slice(&0u32.to_le_bytes());
        bytes[40..44].copy_from_slice(&0u32.to_le_bytes());
        let (file, path) = temp_wav("patched.wav", &bytes);
        let repair = {
            let mut probe_file = File::open(&path).expect("reopen");
            scan(&mut probe_file).expect("repair needed")
        };
        assert_eq!(repair.patches.len(), 2, "riff + data lengths patched");
        let mut src = PatchedSource::new(file, repair.patches);

        // Chunk headers read back through the patched source are corrected.
        src.seek(SeekFrom::Start(36)).expect("seek");
        let mut hdr = [0u8; 8];
        src.read_exact(&mut hdr).expect("read");
        assert_eq!(&hdr[4..8], &128u32.to_le_bytes(), "data len patched");

        // A large unaligned read that straddles both patch ranges.
        src.seek(SeekFrom::Start(0)).expect("seek");
        let mut all = [0u8; 64];
        src.read_exact(&mut all).expect("read");
        assert_eq!(&all[4..8], &164u32.to_le_bytes(), "riff len patched");
        assert_eq!(&all[40..44], &128u32.to_le_bytes(), "data len patched");

        cleanup(path);
    }

    fn cleanup(path: std::path::PathBuf) {
        std::fs::remove_file(path).ok();
    }
}
