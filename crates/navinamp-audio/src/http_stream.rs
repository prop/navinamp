//! A blocking HTTP media source: [`Read`] + [`Seek`] over a remote URL.
//!
//! This is the transport for Navidrome/Subsonic streaming (M5). It lives on
//! the **producer thread only**, wrapped by symphonia's `MediaSourceStream`
//! exactly like a local [`std::fs::File`]; the audio callback never touches
//! it, so a slow network read can stall decoding (audible as an underrun
//! once the 2 s ring runs dry) but can never block the device interrupt.
//!
//! ## Range vs. spill
//!
//! Symphonia's readers need seeking (probe-rewind, container indexes,
//! user seeks). Two server classes are supported, decided by the response
//! to the initial `Range: bytes=0-` GET:
//!
//! - **Range-capable** (answered `206 Partial Content`): every seek issues
//!   `GET` + `Range: bytes=N-` and reads stream straight off the socket.
//!   This is the direct-file path of Navidrome (`format=original`).
//! - **Range-ignoring** (answered `200` with the full body — transcodes):
//!   sequential reads stream off the initial body; on the **first seek**
//!   the full body is downloaded to a spill file in
//!   [`std::env::temp_dir`] (`navinamp-http-spill-*.tmp`) and all subsequent
//!   reads/seeks are served from that file. The spill is deleted when the
//!   stream is dropped (the producer drops it at track swap/stop).
//!
//! Redirects are followed up to 5 hops (the client refuses beyond that).
//!
//! ## Credential hygiene
//!
//! Subsonic stream URLs embed the auth token (`?u=…&t=…&s=…`). reqwest's
//! error messages embed the request URL, so every error that crosses this
//! module has the URL scrubbed to `<url>` before it is surfaced — engine
//! `Event::Error`s and logs must never carry credentials.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Redirect hop limit (matches the contract in the crate docs).
const MAX_REDIRECTS: usize = 5;
/// TCP connect timeout. There is deliberately **no** whole-request timeout:
/// a streamed body legitimately lives for the whole track.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Parsed `Content-Range: bytes <start>-<end>/<total>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ContentRange {
    pub start: u64,
    pub end: u64,
    pub total: u64,
}

/// One ranged GET response. Transport-agnostic so unit tests can fake the
/// server (the integration tests use real sockets against `127.0.0.1`).
pub(crate) struct HttpResponse {
    pub status: u16,
    /// `Content-Length` (length of *this response's body*, not the file).
    pub content_length: Option<u64>,
    /// `Content-Range` (present on conforming `206`s).
    pub content_range: Option<ContentRange>,
    /// Response body, positioned at `content_range.start` (206) or `0` (200).
    pub body: Box<dyn Read + Send>,
}

/// The inner transport: one ranged GET. Production wraps
/// `reqwest::blocking`; the seam exists so the range/spill state machine is
/// unit-testable without sockets.
///
/// `Sync` because symphonia's `MediaSource` supertrait chain requires it
/// (the stream is still only ever *used* from the producer thread; the
/// mutex is uncontended).
pub(crate) trait HttpFetch: Send + Sync {
    /// GET `url` requesting `Range: bytes=<range_start>-`. Redirects are
    /// followed (bounded); the returned response is the final one.
    fn get(&self, url: &str, range_start: u64) -> io::Result<HttpResponse>;
}

/// Production transport over `reqwest::blocking` (rustls).
pub(crate) struct ReqwestFetch {
    client: reqwest::blocking::Client,
}

impl ReqwestFetch {
    pub(crate) fn new() -> io::Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(scrub_reqwest_error)?;
        Ok(Self { client })
    }
}

impl HttpFetch for ReqwestFetch {
    fn get(&self, url: &str, range_start: u64) -> io::Result<HttpResponse> {
        let resp = self
            .client
            .get(url)
            .header("Range", format!("bytes={range_start}-"))
            .send()
            .map_err(scrub_reqwest_error)?;

        let status = resp.status().as_u16();
        let header = |name: &str| -> Option<String> {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.trim().to_string())
        };
        let content_length = header("Content-Length").and_then(|v| v.parse().ok());
        let content_range = header("Content-Range").and_then(|v| parse_content_range(&v));

        Ok(HttpResponse {
            status,
            content_length,
            content_range,
            body: Box::new(resp),
        })
    }
}

/// Parses `bytes <start>-<end>/<total>` (`*` total is rejected — we need a
/// concrete length for seeking).
fn parse_content_range(value: &str) -> Option<ContentRange> {
    let (range, total) = value.split_once('/')?;
    let total: u64 = total.parse().ok()?;
    let (start, end) = range.trim().strip_prefix("bytes ")?.split_once('-')?;
    Some(ContentRange {
        start: start.trim().parse().ok()?,
        end: end.trim().parse().ok()?,
        total,
    })
}

/// reqwest error → `io::Error` with the request URL scrubbed out (it may
/// embed the Subsonic token; see the module docs).
fn scrub_reqwest_error(e: reqwest::Error) -> io::Error {
    let mut msg = e.to_string();
    if let Some(url) = e.url() {
        msg = msg.replace(url.as_str(), "<url>");
    }
    io::Error::other(msg)
}

/// A `Read` body hidden behind a mutex so the owning stream is `Sync`
/// (required by symphonia's `MediaSource`). Uncontended in practice: the
/// producer thread is the only reader.
struct SyncBody(Mutex<Box<dyn Read + Send>>);

impl SyncBody {
    fn new(body: Box<dyn Read + Send>) -> Self {
        Self(Mutex::new(body))
    }

    fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.lock().unwrap().read(buf)
    }
}

/// A seekable byte stream over a remote HTTP resource. See the module docs
/// for the range/spill behavior.
pub(crate) struct HttpStream<F: HttpFetch = ReqwestFetch> {
    url: String,
    fetch: F,
    /// Total resource length in bytes (always known — `Content-Length` or
    /// `Content-Range` total is required up front).
    total: u64,
    /// Absolute position of the next byte to read.
    pos: u64,
    body: Body,
    /// Spill file path, kept so [`Drop`] can delete it.
    spill: Option<PathBuf>,
}

enum Body {
    /// A live HTTP body whose byte 0 is the file offset `self.pos` (the
    /// variant no longer carries the offset — `pos` is the single source of
    /// truth once a body is installed).
    Remote { body: SyncBody },
    /// Range-ignoring server: the whole resource, in a local temp file.
    Spilled(File),
    /// Logically positioned at/after EOF (nothing to read until a seek).
    Exhausted,
}

impl<F: HttpFetch> HttpStream<F> {
    /// Open `url`, fetching through `fetch` (tests inject a fake here).
    pub(crate) fn with_fetch(url: &str, fetch: F) -> io::Result<Self> {
        let resp = fetch.get(url, 0)?;
        let (total, body) = match resp.status {
            206 => {
                let cr = resp.content_range.as_ref().ok_or_else(|| {
                    io::Error::other("HTTP 206 without Content-Range (cannot determine length)")
                })?;
                if cr.start != 0 {
                    return Err(io::Error::other(format!(
                        "server answered `Range: bytes=0-` with a body starting at {}",
                        cr.start
                    )));
                }
                (
                    cr.total,
                    Body::Remote {
                        body: SyncBody::new(resp.body),
                    },
                )
            }
            200 => {
                let len = resp.content_length.ok_or_else(|| {
                    io::Error::other("HTTP 200 without Content-Length (cannot determine length)")
                })?;
                (
                    len,
                    Body::Remote {
                        body: SyncBody::new(resp.body),
                    },
                )
            }
            status => return Err(http_status_error(status)),
        };
        if total == 0 {
            return Err(io::Error::other("remote media is empty (Content-Length 0)"));
        }
        Ok(Self {
            url: url.to_owned(),
            fetch,
            total,
            pos: 0,
            body,
            spill: None,
        })
    }

    /// Seek to an absolute byte offset, re-establishing the body:
    /// ranged GET for capable servers, spill for the rest.
    fn seek_absolute(&mut self, abs: u64) -> io::Result<()> {
        if abs == self.pos {
            return Ok(());
        }
        if abs >= self.total {
            self.body = Body::Exhausted;
            self.pos = abs;
            return Ok(());
        }

        if let Body::Spilled(file) = &mut self.body {
            file.seek(SeekFrom::Start(abs))?;
            self.pos = abs;
            return Ok(());
        }

        let resp = self.fetch.get(&self.url, abs)?;
        match resp.status {
            206 => {
                let cr = resp.content_range.as_ref().ok_or_else(|| {
                    io::Error::other("HTTP 206 without Content-Range (cannot determine length)")
                })?;
                self.total = cr.total;
                let mut body = resp.body;
                // A well-behaved server returns exactly the asked range;
                // tolerate block-clamped starts by discarding the excess.
                if cr.start > abs {
                    return Err(io::Error::other(format!(
                        "server returned Content-Range starting at {} for a seek to {abs}",
                        cr.start
                    )));
                }
                let mut skip = abs - cr.start;
                let mut scratch = [0u8; 4096];
                while skip > 0 {
                    let want = skip.min(scratch.len() as u64) as usize;
                    let n = read_exactish(&mut body, &mut scratch[..want])?;
                    if n == 0 {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "http body ended before the seek target",
                        ));
                    }
                    skip -= n as u64;
                }
                self.body = Body::Remote {
                    body: SyncBody::new(body),
                };
                self.pos = abs;
                Ok(())
            }
            // The server ignored the Range (transcodes): take the full body
            // to a spill file and serve everything locally from now on.
            200 => self.spill(resp, abs),
            status => Err(http_status_error(status)),
        }
    }

    /// Download `resp`'s (full) body to a temp file, then seek it to `abs`.
    fn spill(&mut self, resp: HttpResponse, abs: u64) -> io::Result<()> {
        let declared = resp.content_length.ok_or_else(|| {
            io::Error::other("HTTP 200 without Content-Length (cannot determine length)")
        })?;
        let path = spill_path();
        // read+write (not `File::create`, which is write-only): the spilled
        // file is read back through this same handle.
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .map_err(|e| io::Error::other(format!("cannot create spill file: {e}")))?;
        let mut body = resp.body;
        let written = match std::io::copy(&mut body, &mut file) {
            Ok(n) => n,
            Err(e) => {
                let _ = std::fs::remove_file(&path);
                return Err(e);
            }
        };
        if written < declared {
            let _ = std::fs::remove_file(&path);
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("spill: short body ({written} of {declared} bytes)"),
            ));
        }
        file.seek(SeekFrom::Start(abs))?;
        self.total = declared;
        self.body = Body::Spilled(file);
        self.spill = Some(path);
        self.pos = abs;
        Ok(())
    }
}

/// `HttpStream<ReqwestFetch>` opened against a real URL.
impl HttpStream<ReqwestFetch> {
    pub(crate) fn open(url: &str) -> io::Result<Self> {
        Self::with_fetch(url, ReqwestFetch::new()?)
    }
}

/// Reads up to `buf.len()`, looping only on `ErrorKind::Interrupted`.
fn read_exactish(body: &mut Box<dyn Read + Send>, buf: &mut [u8]) -> io::Result<usize> {
    loop {
        match body.read(buf) {
            Ok(0) => return Ok(0),
            Ok(n) => return Ok(n),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
}

fn http_status_error(status: u16) -> io::Error {
    io::Error::other(format!("HTTP {status}"))
}

/// Unique spill file path under the system temp dir.
fn spill_path() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("navinamp-http-spill-{}-{n}.tmp", std::process::id()))
}

impl<F: HttpFetch> Drop for HttpStream<F> {
    fn drop(&mut self) {
        if let Some(path) = self.spill.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl<F: HttpFetch> Read for HttpStream<F> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.total {
            return Ok(0);
        }
        match &mut self.body {
            Body::Remote { body, .. } => loop {
                match body.read(buf) {
                    Ok(n) => {
                        self.pos += n as u64;
                        if n == 0 && self.pos < self.total && !buf.is_empty() {
                            return Err(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                "http body ended before the advertised length",
                            ));
                        }
                        return Ok(n);
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                }
            },
            Body::Spilled(file) => {
                let n = file.read(buf)?;
                self.pos += n as u64;
                Ok(n)
            }
            Body::Exhausted => Ok(0),
        }
    }
}

impl<F: HttpFetch> Seek for HttpStream<F> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let abs = match pos {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
            SeekFrom::End(d) => self.total.checked_add_signed(d),
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek out of range"))?;
        self.seek_absolute(abs)?;
        Ok(abs)
    }
}

/// Symphonia's media source: the stream is always seekable (ranged GETs or
/// the spill file) and its length is exact, which lets the WAV/MP3 readers
/// compute durations without scanning.
impl<F: HttpFetch + 'static> symphonia::core::io::MediaSource for HttpStream<F> {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::Mutex;

    /// In-memory transport: serves `data` either with real 206s or always
    /// with full-body 200s (transcode behavior). Records requested offsets.
    struct FakeFetch {
        data: Vec<u8>,
        honor_ranges: bool,
        requests: Mutex<Vec<u64>>,
        status: Option<u16>,
    }

    impl FakeFetch {
        fn ranged(data: Vec<u8>) -> Self {
            Self {
                data,
                honor_ranges: true,
                requests: Mutex::new(Vec::new()),
                status: None,
            }
        }

        fn full_only(data: Vec<u8>) -> Self {
            Self {
                data,
                honor_ranges: false,
                requests: Mutex::new(Vec::new()),
                status: None,
            }
        }

        fn failing(status: u16) -> Self {
            Self {
                data: Vec::new(),
                honor_ranges: false,
                requests: Mutex::new(Vec::new()),
                status: Some(status),
            }
        }

        fn body(&self, start: u64) -> Box<dyn Read + Send> {
            Box::new(Cursor::new(self.data[start as usize..].to_vec()))
        }
    }

    impl HttpFetch for FakeFetch {
        fn get(&self, _url: &str, range_start: u64) -> io::Result<HttpResponse> {
            self.requests.lock().unwrap().push(range_start);
            if let Some(status) = self.status {
                return Ok(HttpResponse {
                    status,
                    content_length: Some(0),
                    content_range: None,
                    body: Box::new(Cursor::new(Vec::new())),
                });
            }
            let len = self.data.len() as u64;
            if self.honor_ranges {
                let start = range_start.min(len);
                Ok(HttpResponse {
                    status: 206,
                    content_length: Some(len - start),
                    content_range: Some(ContentRange {
                        start,
                        end: len - 1,
                        total: len,
                    }),
                    body: self.body(start),
                })
            } else {
                Ok(HttpResponse {
                    status: 200,
                    content_length: Some(len),
                    content_range: None,
                    body: self.body(0),
                })
            }
        }
    }

    fn data() -> Vec<u8> {
        (0..1000u32).map(|i| (i % 251) as u8).collect()
    }

    fn read_all(stream: &mut HttpStream<FakeFetch>, from: u64, n: u64) -> io::Result<Vec<u8>> {
        stream.seek(SeekFrom::Start(from))?;
        let mut out = Vec::with_capacity(n as usize);
        let mut take = (&mut *stream).take(n);
        take.read_to_end(&mut out)?;
        Ok(out)
    }

    #[test]
    fn range_capable_fetch_streams_and_seeks() {
        use symphonia::core::io::MediaSource as _;

        let fetch = FakeFetch::ranged(data());
        let mut stream = HttpStream::with_fetch("http://x/media", fetch).unwrap();
        assert_eq!(stream.byte_len(), Some(1000));
        assert!(stream.is_seekable());

        // Sequential read from the initial body.
        let head = read_all(&mut stream, 0, 16).unwrap();
        assert_eq!(head, data()[..16]);

        // Forward seek -> new ranged GET, correct slice.
        let mid = read_all(&mut stream, 500, 10).unwrap();
        assert_eq!(mid, data()[500..510]);

        // Backward seek works too (fresh GET).
        let back = read_all(&mut stream, 10, 10).unwrap();
        assert_eq!(back, data()[10..20]);

        // Seek-from-end and tell().
        stream.seek(SeekFrom::End(-4)).unwrap();
        let mut tail = Vec::new();
        stream.read_to_end(&mut tail).unwrap();
        assert_eq!(tail, data()[996..]);

        // Requests: initial open + one per non-trivial seek.
        let reqs = { stream.fetch.requests.lock().unwrap().clone() };
        assert_eq!(reqs, vec![0, 500, 10, 996]);
    }

    #[test]
    fn range_ignoring_server_spills_on_first_seek() {
        let fetch = FakeFetch::full_only(data());
        let mut stream = HttpStream::with_fetch("http://x/media", fetch).unwrap();

        // Sequential reads stream off the initial 200 body (no spill yet).
        let mut head = vec![0u8; 16];
        stream.read_exact(&mut head).unwrap();
        assert_eq!(head, data()[..16]);
        assert!(stream.spill.is_none());

        // First seek: ranged GET comes back 200 -> spill, serve from file.
        let mid = read_all(&mut stream, 700, 12).unwrap();
        assert_eq!(mid, data()[700..712]);
        assert!(stream.spill.is_some(), "spill file must exist");

        // Post-spill seeks never hit the transport again.
        let again = read_all(&mut stream, 3, 9).unwrap();
        assert_eq!(again, data()[3..12]);
        let reqs = { stream.fetch.requests.lock().unwrap().clone() };
        assert_eq!(
            reqs,
            vec![0, 700],
            "post-spill seeks must be local: {reqs:?}"
        );

        // Drop removes the spill file.
        let path = stream.spill.clone().unwrap();
        assert!(path.exists());
        drop(stream);
        assert!(!path.exists(), "spill file must be deleted on drop");
    }

    #[test]
    fn http_error_status_is_an_open_error() {
        let fetch = FakeFetch::failing(404);
        let err = HttpStream::with_fetch("http://x/missing", fetch)
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("404"), "{err}");
    }

    #[test]
    fn seek_past_eof_reads_zero() {
        let fetch = FakeFetch::ranged(data());
        let mut stream = HttpStream::with_fetch("http://x/media", fetch).unwrap();
        stream.seek(SeekFrom::Start(5000)).unwrap();
        let mut buf = [0u8; 8];
        assert_eq!(stream.read(&mut buf).unwrap(), 0);
        // And back in range again.
        let back = read_all(&mut stream, 990, 10).unwrap();
        assert_eq!(back, data()[990..]);
    }

    #[test]
    fn content_range_parser() {
        let cr = parse_content_range("bytes 0-499/1234").unwrap();
        assert_eq!(
            cr,
            ContentRange {
                start: 0,
                end: 499,
                total: 1234
            }
        );
        assert!(parse_content_range("bytes 0-499/*").is_none());
        assert!(parse_content_range("garbage").is_none());
        assert!(parse_content_range("bytes 500-999/1000, junk").is_none());
    }
}
