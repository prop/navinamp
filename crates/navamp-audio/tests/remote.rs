//! Remote-streaming integration tests (M5): [`TrackSource::Remote`] against
//! an in-process HTTP/1.1 fixture server bound to 127.0.0.1 — no external
//! network. The fixture speaks just enough HTTP for the reqwest transport:
//! one request per connection (`Connection: close`), `Range` honored per
//! [`Mode`].

mod common;

use std::io::{Read as _, Write as _};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, TryRecvError};
use navamp_audio::{Command, EngineHandle, Event, PlaybackStatus, TestSink, TrackSource};

const RATE: u32 = 44100;
const TIMEOUT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// HTTP fixture server
// ---------------------------------------------------------------------------

/// How the fixture answers requests.
#[derive(Clone, Copy)]
enum Mode {
    /// Proper `206` + `Content-Range` answers (direct file streaming).
    Ranges,
    /// Ignores `Range`, always `200` with the full body (transcode behavior).
    FullOnly,
    /// Always `404`.
    NotFound,
}

/// Recorded requests: the raw `Range` header value per request (`None` when
/// absent).
type Seen = Arc<Mutex<Vec<Option<String>>>>;

struct Server {
    addr: SocketAddr,
    requests: Seen,
}

impl Server {
    fn start(mode: Mode, body: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
        let addr = listener.local_addr().expect("addr");
        let requests: Seen = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&requests);
        std::thread::Builder::new()
            .name("http-fixture".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { break };
                    if handle(&mut stream, mode, &body, &seen).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn fixture server");
        Self { addr, requests }
    }

    fn url(&self) -> String {
        format!("http://{}/stream?id=t1", self.addr)
    }

    fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

/// Serve exactly one request on `stream`, then close.
fn handle(stream: &mut TcpStream, mode: Mode, body: &[u8], seen: &Seen) -> std::io::Result<()> {
    let request = read_request(stream)?;
    let range = request.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("range")
            .then(|| value.trim().to_string())
    });
    seen.lock().unwrap().push(range.clone());

    match mode {
        Mode::NotFound => {
            write_simple(stream, 404, "Not Found", b"missing")?;
        }
        Mode::Ranges => {
            let start = range
                .as_deref()
                .and_then(|r| r.strip_prefix("bytes="))
                .and_then(|r| r.split_once('-'))
                .and_then(|(s, _)| s.trim().parse::<u64>().ok())
                .unwrap_or(0)
                .min(body.len() as u64) as usize;
            // A range-capable server answers any Range request with 206 —
            // even `bytes=0-`.
            let head = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Type: audio/wav\r\n\
                 Content-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\n\
                 Accept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                body.len() - start,
                start,
                body.len() - 1,
                body.len()
            );
            stream.write_all(head.as_bytes())?;
            stream.write_all(&body[start..])?;
        }
        Mode::FullOnly => {
            write_simple(stream, 200, "OK", body)?;
        }
    }
    stream.flush()?;
    stream.shutdown(Shutdown::Both)?;
    Ok(())
}

/// Reads request headers (up to `\r\n\r\n`). GETs have no body.
fn read_request(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut buf = Vec::with_capacity(512);
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") && buf.len() < 8192 {
        if stream.read(&mut byte)? == 0 {
            break;
        }
        buf.push(byte[0]);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn write_simple(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: audio/wav\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)
}

// ---------------------------------------------------------------------------
// engine drivers (mirrors tests/engine.rs)
// ---------------------------------------------------------------------------

fn engine_on(sink: &TestSink) -> EngineHandle {
    EngineHandle::start_with_sink(sink.clone()).expect("engine start")
}

fn drive(sink: &TestSink, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while !cond() {
        assert!(
            Instant::now() < deadline,
            "drive: condition not met in time"
        );
        sink.pull(128);
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn wait_event(
    sink: &TestSink,
    rx: &Receiver<Event>,
    mut pred: impl FnMut(&Event) -> bool,
) -> Event {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Ok(e) = rx.try_recv() {
            if pred(&e) {
                return e;
            }
            continue;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for matching event"
        );
        sink.pull(128);
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn start_and_get_playing(engine: &EngineHandle, sink: &TestSink, rx: &Receiver<Event>) {
    wait_event(sink, rx, |e| matches!(e, Event::TrackStarted { .. }));
    drive(sink, || engine.status() == PlaybackStatus::Playing);
}

fn pull_frames(sink: &TestSink, out: &mut Vec<f32>, frames: usize) {
    let mut remaining = frames;
    while remaining > 0 {
        let n = remaining.min(512);
        out.extend(sink.pull(n));
        remaining -= n;
    }
}

fn assert_close(actual: &[f32], expected: &[f32], tol: f64, ctx: &str) {
    assert_eq!(actual.len(), expected.len(), "{ctx}: length mismatch");
    for (i, (a, e)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (f64::from(*a) - f64::from(*e)).abs() <= tol,
            "{ctx}: sample {i}: {a} vs {e}"
        );
    }
}

fn drain_queue(engine: &EngineHandle, sink: &TestSink, rx: &Receiver<Event>) {
    wait_event(sink, rx, |e| matches!(e, Event::EndOfQueue));
    drive(sink, || engine.status() == PlaybackStatus::Stopped);
}

/// Underrun budget for tests that pull through the queue end: the engine
/// can only flip to `Stopped` *after* the last frame was consumed, so a
/// pull landing in that flip window counts a few frames of "Playing with
/// an empty ring" — an artifact of pulling past the end, not starvation
/// (which shows up as hundreds+ of frames). The gapless test, which stops
/// pulling at the exact audio end, keeps the strict `== 0`.
fn assert_no_starved_underrun(engine: &EngineHandle) {
    let underruns = engine.underrun_frames();
    assert!(
        underruns < 300,
        "underruns {underruns} frames: the stream starved mid-track"
    );
}

/// Resume after a paused seek and verify the output continues exactly at
/// `seek_frame` of `expected` through the end of the track.
///
/// Race-free by construction: every pull from the moment `Resume` is sent
/// is recorded (poll pulls that straddle the status flip consume real
/// audio; discarding them would skew the alignment), the post-seek audio
/// is located inside the recording by window-matching, and the remainder
/// must continue sample-exactly from there.
fn resume_and_expect_continuation(
    engine: &EngineHandle,
    sink: &TestSink,
    rx: &Receiver<Event>,
    expected: &[f32],
    seek_frame: usize,
    ctx: &str,
) {
    engine.send(Command::Resume).expect("resume");
    let mut tail = Vec::new();
    let deadline = Instant::now() + TIMEOUT;
    while engine.status() != PlaybackStatus::Playing {
        assert!(Instant::now() < deadline, "{ctx}: never resumed");
        tail.extend(sink.pull(64));
        std::thread::sleep(Duration::from_millis(1));
    }
    // The remainder of the track, through the end of the queue. Events are
    // checked BEFORE each pull: `EndOfQueue` is only emitted after the
    // status flipped to Stopped, so this never pulls "past the end" while
    // still Playing (which would count a phantom tail underrun).
    loop {
        match rx.try_recv() {
            Ok(Event::EndOfQueue) => break,
            Ok(_) => {}
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => panic!("{ctx}: event channel closed"),
        }
        tail.extend(sink.pull(256));
        std::thread::sleep(Duration::from_millis(1));
    }
    drive(sink, || engine.status() == PlaybackStatus::Stopped);

    let left: Vec<f32> = tail.iter().step_by(2).copied().collect();
    let exp_left: Vec<f32> = expected.iter().step_by(2).copied().collect();
    let mut aligned_at = None;
    for start in 0..left.len().saturating_sub(256) {
        let err: f64 = (0..256)
            .map(|f| {
                let d = f64::from(left[start + f] - exp_left[seek_frame + f]);
                d * d
            })
            .sum::<f64>()
            / 256.0;
        if err < 1e-8 {
            aligned_at = Some(start);
            break;
        }
    }
    let Some(start) = aligned_at else {
        panic!(
            "{ctx}: post-seek audio never reached the seek target; first samples {:?}",
            &left[..8.min(left.len())]
        );
    };
    assert!(
        start < 1500,
        "{ctx}: too much dead air before the post-seek audio: {start} frames"
    );
    // Exact continuation from the alignment to the track end.
    let want = expected.len() - seek_frame * 2;
    assert!(
        tail.len() >= start * 2 + want,
        "{ctx}: post-seek audio too short: {} of {} samples",
        tail.len() - start * 2,
        want
    );
    assert_close(
        &tail[start * 2..start * 2 + want],
        &expected[seek_frame * 2..],
        2e-4,
        ctx,
    );
}

// ---------------------------------------------------------------------------
// (a) range-capable server: decode == local decode, seek exact
// ---------------------------------------------------------------------------

#[test]
fn range_capable_remote_matches_local_and_seeks_exactly() {
    let (wav, expected) = common::stereo_sine_fixture("remote-ranged", RATE, 0.5, 440.0);
    let body = std::fs::read(&wav).expect("read fixture wav");
    let server = Server::start(Mode::Ranges, body);

    let sink = TestSink::new(RATE);
    let engine = engine_on(&sink);
    let rx = engine.subscribe();

    // Local reference: play the same file, capture the whole track.
    engine
        .send(Command::Play(TrackSource::File(wav.clone())))
        .expect("play local");
    start_and_get_playing(&engine, &sink, &rx);
    let mut local = Vec::new();
    pull_frames(&sink, &mut local, 22_050);
    drain_queue(&engine, &sink, &rx);

    // Remote: whole track; output must match the local decode exactly.
    engine
        .send(Command::Play(TrackSource::Remote { url: server.url() }))
        .expect("play remote");
    start_and_get_playing(&engine, &sink, &rx);
    let mut remote = Vec::new();
    pull_frames(&sink, &mut remote, 22_050);
    assert_eq!(remote, local, "remote decode must equal local decode");
    drain_queue(&engine, &sink, &rx);

    // Seek on the remote stream (paused): the engine re-issues a ranged GET
    // and decoding continues exactly at the target frame.
    engine
        .send(Command::Play(TrackSource::Remote { url: server.url() }))
        .expect("play remote again");
    start_and_get_playing(&engine, &sink, &rx);
    let mut head = Vec::new();
    pull_frames(&sink, &mut head, 4410);
    assert_close(&head, &expected[..4410 * 2], 1e-9, "remote head");

    engine.send(Command::Pause).expect("pause");
    drive(&sink, || engine.status() == PlaybackStatus::Paused);
    engine.send(Command::Seek(0.3)).expect("seek");
    drive(&sink, || engine.position_secs() >= 0.295);
    let p = engine.position_secs();
    assert!((p - 0.3).abs() < 0.005, "position after remote seek {p}");
    resume_and_expect_continuation(&engine, &sink, &rx, &expected, 13_230, "post-seek remote");

    assert_no_starved_underrun(&engine);
    // Initial open + probe rewinds + the seek: at least three requests.
    assert!(
        server.request_count() >= 3,
        "expected several ranged requests, saw {}",
        server.request_count()
    );
}

// ---------------------------------------------------------------------------
// (b) range-ignoring server: spill on first seek, whole track still plays
// ---------------------------------------------------------------------------

#[test]
fn range_ignoring_server_spills_on_seek_and_plays_whole_track() {
    let (wav, expected) = common::stereo_sine_fixture("remote-fullonly", RATE, 0.5, 660.0);
    let body = std::fs::read(&wav).expect("read fixture wav");
    let server = Server::start(Mode::FullOnly, body);

    let sink = TestSink::new(RATE);
    let engine = engine_on(&sink);
    let rx = engine.subscribe();

    engine
        .send(Command::Play(TrackSource::Remote { url: server.url() }))
        .expect("play remote");
    start_and_get_playing(&engine, &sink, &rx);

    // Sequential reads stream straight off the 200 body.
    let mut head = Vec::new();
    pull_frames(&sink, &mut head, 2205);
    assert_close(
        &head,
        &expected[..2205 * 2],
        1e-9,
        "pre-seek streamed audio",
    );

    // Seek: the ranged GET comes back 200 -> the engine spills the full body
    // to a temp file and serves the seek (and everything after) locally.
    engine.send(Command::Pause).expect("pause");
    drive(&sink, || engine.status() == PlaybackStatus::Paused);
    engine.send(Command::Seek(0.3)).expect("seek");
    drive(&sink, || engine.position_secs() >= 0.295);
    assert!(
        server.request_count() >= 2,
        "the seek must have issued a second request, saw {}",
        server.request_count()
    );

    resume_and_expect_continuation(&engine, &sink, &rx, &expected, 13_230, "post-spill");
    assert_no_starved_underrun(&engine);
}

// ---------------------------------------------------------------------------
// (c) 404: Error event, engine survives
// ---------------------------------------------------------------------------

#[test]
fn http_404_surfaces_error_and_engine_survives() {
    let server = Server::start(Mode::NotFound, Vec::new());
    let (wav, expected) = common::stereo_sine_fixture("remote-after-404", RATE, 0.25, 440.0);

    let sink = TestSink::new(RATE);
    let engine = engine_on(&sink);
    let rx = engine.subscribe();

    // From idle: error, stays stopped.
    engine
        .send(Command::Play(TrackSource::Remote { url: server.url() }))
        .expect("send play");
    let e = wait_event(&sink, &rx, |e| matches!(e, Event::Error { .. }));
    match e {
        Event::Error { id, msg } => {
            assert_eq!(id, Some(1));
            assert!(
                msg.contains("404"),
                "message should mention the status: {msg}"
            );
        }
        other => panic!("expected Error, got {other:?}"),
    }
    assert_eq!(engine.status(), PlaybackStatus::Stopped);

    // A bad enqueue while playing: error now, current track still finishes.
    engine
        .send(Command::Play(TrackSource::File(wav)))
        .expect("play local");
    start_and_get_playing(&engine, &sink, &rx);
    engine
        .send(Command::EnqueueNext(TrackSource::Remote {
            url: server.url(),
        }))
        .expect("enqueue bad remote");
    let e2 = wait_event(&sink, &rx, |e| matches!(e, Event::Error { .. }));
    // ids: 1 = failed Play, 2 = local track, 3 = failed EnqueueNext.
    assert!(matches!(e2, Event::Error { id: Some(3), .. }), "{e2:?}");
    drain_queue(&engine, &sink, &rx);

    // The engine is still fully functional afterwards.
    let (wav2, expected2) = common::stereo_sine_fixture("remote-after-404-b", RATE, 0.25, 880.0);
    let _ = expected;
    engine
        .send(Command::Play(TrackSource::File(wav2)))
        .expect("play again");
    start_and_get_playing(&engine, &sink, &rx);
    let mut out = Vec::new();
    pull_frames(&sink, &mut out, 11_025);
    assert_close(&out, &expected2, 1e-9, "post-error playback");
    drain_queue(&engine, &sink, &rx);
}

// ---------------------------------------------------------------------------
// (d) gapless local -> remote transition
// ---------------------------------------------------------------------------

#[test]
fn gapless_local_to_remote_transition_is_sample_exact() {
    let (wav_a, exp_a) = common::stereo_sine_fixture("gapless-l2r-a", RATE, 0.5, 440.0);
    let (wav_b, exp_b) = common::stereo_sine_fixture("gapless-l2r-b", RATE, 0.5, 660.0);
    let body = std::fs::read(&wav_b).expect("read fixture wav");
    let server = Server::start(Mode::Ranges, body);

    let sink = TestSink::new(RATE);
    let engine = engine_on(&sink);
    let rx = engine.subscribe();

    engine
        .send(Command::Play(TrackSource::File(wav_a)))
        .expect("play local a");
    engine
        .send(Command::EnqueueNext(TrackSource::Remote {
            url: server.url(),
        }))
        .expect("enqueue remote b");

    start_and_get_playing(&engine, &sink, &rx);

    // This window must be pure audio: A ++ B exactly.
    let mut out = Vec::new();
    pull_frames(&sink, &mut out, 22_050 + 22_050);
    let mut expected = exp_a;
    expected.extend_from_slice(&exp_b);
    assert_close(&out, &expected, 1e-9, "gapless local->remote");

    let finished = wait_event(&sink, &rx, |e| matches!(e, Event::TrackFinished { id: 1 }));
    let _ = finished;
    wait_event(&sink, &rx, |e| {
        matches!(e, Event::TrackStarted { id: 2, .. })
    });
    drain_queue(&engine, &sink, &rx);
    assert_eq!(engine.underrun_frames(), 0, "gapless must not underrun");

    let tail = sink.pull(256);
    assert!(tail.iter().all(|&s| s == 0.0), "silence after EndOfQueue");
    let _ = TryRecvError::Empty;
}
