//! A single-producer / single-consumer lock-free ring buffer for interleaved f32 audio.
//!
//! The **producer** is the decode thread ([`crate::producer`]); the **consumer** is the
//! real-time output callback ([`crate::callback`]). Both halves are non-blocking: pushes
//! and pops are plain atomic loads/stores plus `memcpy`; no locks, no allocation, no
//! syscalls — safe to call from an audio interrupt.
//!
//! # Reset protocol (seek / stop / track replacement)
//!
//! The producer occasionally needs to discard buffered-but-unplayed audio *instantly*
//! (a seek must not play out another second of stale samples). The consumer owns the
//! read cursor, so the producer cannot clear the buffer itself. Instead:
//!
//! 1. The producer bumps a **generation counter**.
//! 2. On its next pull, the consumer notices the new generation and *fast-forwards*
//!    its read cursor to the write cursor — an O(1) discard. The discarded samples are
//!    reported back so the caller can account them into the samples-played counter
//!    (this keeps the producer/consumer frame counters FIFO-aligned; see
//!    [`crate::shared`]).
//! 3. The producer waits (bounded) until the buffer reports empty before writing new
//!    audio. If the wait times out (device stopped pulling), it proceeds anyway — the
//!    consumer will simply discard a little of the new audio on its next wake, which is
//!    treated as a spurious underrun and is always memory-safe.
//!
//! Frame granularity: all pushes and pops move in whole frames (multiples of the
//! channel count). Both cursors therefore stay frame-aligned by induction, and a
//! partial push can never split a stereo pair.

use std::cell::UnsafeCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// An atomic padded to its own cache line to avoid false sharing between the
/// producer and consumer cursors.
#[repr(C, align(64))]
struct PaddedAtomicU64(AtomicU64);

/// # Safety
/// The sample buffer is written **only** by the producer (cells ahead of `tail`)
/// and read **only** by the consumer (cells behind `tail`, ahead of `head`).
/// Those two cell ranges are disjoint at all times, so sharing an `Inner`
/// across two threads cannot race on the buffer. The cursors are atomics.
struct Inner {
    buf: UnsafeCell<Box<[f32]>>,
    mask: u64,
    /// Samples ever popped (including discarded-on-reset samples). Consumer-owned.
    head: PaddedAtomicU64,
    /// Samples ever pushed. Producer-owned.
    tail: PaddedAtomicU64,
    /// Reset generation. Producer bumps, consumer observes.
    generation: PaddedAtomicU64,
    /// Last generation the consumer has observed (the reset ack). Consumer-owned.
    acked_generation: PaddedAtomicU64,
}

unsafe impl Sync for Inner {}

/// Producer half of the ring buffer.
pub(crate) struct RingProducer {
    inner: Arc<Inner>,
    /// Cached copy of `head` to avoid cache-line ping-pong on every push.
    cached_head: u64,
}

/// Consumer half of the ring buffer.
pub(crate) struct RingConsumer {
    inner: Arc<Inner>,
    /// Cached copy of `tail` (refreshed only when the ring looks empty).
    cached_tail: u64,
    /// Last observed reset generation.
    cached_gen: u64,
}

/// Create a ring buffer holding at least `frames_hint` frames of `channels`-channel
/// interleaved audio. Actual capacity is rounded up to a power of two.
pub(crate) fn ring_buffer(frames_hint: u64, channels: u16) -> (RingProducer, RingConsumer) {
    // Frame-granularity invariant (module docs): the cursors stay frame-aligned
    // by induction only if a whole number of frames divides the sample
    // capacity. A power-of-two sample capacity guarantees that for power-of-two
    // channel counts — the engine opens sinks as stereo, so this always holds
    // today; the assert catches a future multichannel change before it splits
    // frames.
    debug_assert!(
        channels > 0 && channels.is_power_of_two(),
        "ring frame alignment requires a power-of-two channel count"
    );
    let samples = (frames_hint * u64::from(channels)).max(4) as usize;
    let cap = samples.next_power_of_two();
    let inner = Inner {
        buf: UnsafeCell::new(vec![0.0; cap].into_boxed_slice()),
        mask: (cap - 1) as u64,
        head: PaddedAtomicU64(AtomicU64::new(0)),
        tail: PaddedAtomicU64(AtomicU64::new(0)),
        generation: PaddedAtomicU64(AtomicU64::new(0)),
        acked_generation: PaddedAtomicU64(AtomicU64::new(0)),
    };
    let arc = Arc::new(inner);
    let generation = arc.generation.0.load(Ordering::Relaxed);
    (
        RingProducer {
            inner: arc.clone(),
            cached_head: 0,
        },
        RingConsumer {
            inner: arc,
            cached_tail: 0,
            cached_gen: generation,
        },
    )
}

impl RingProducer {
    /// Try to append `src`, returning how many samples actually fit.
    /// `src.len()` must be a multiple of the channel count.
    pub(crate) fn push(&mut self, src: &[f32]) -> usize {
        // SAFETY (see Inner): only the producer writes buffer cells, and only in
        // the free region; the borrow ends before any atomic publication.
        let buf = unsafe { &mut *self.inner.buf.get() };
        let cap = buf.len();
        let tail = self.inner.tail.0.load(Ordering::Relaxed);
        let used = tail.wrapping_sub(self.cached_head) as usize;
        let free = cap - used;
        let n = free.min(src.len());
        if n == 0 {
            // Full: refresh the cached consumer cursor for the next attempt.
            self.cached_head = self.inner.head.0.load(Ordering::Acquire);
            return 0;
        }
        let pos = (tail & self.inner.mask) as usize;
        let first = n.min(cap - pos);
        // SAFETY (see Inner): producer writes only cells at/after `tail`, which
        // the consumer is not reading.
        let buf = unsafe { &mut *self.inner.buf.get() };
        buf[pos..pos + first].copy_from_slice(&src[..first]);
        let second = n - first;
        if second > 0 {
            buf[..second].copy_from_slice(&src[first..n]);
        }
        self.inner.tail.0.store(tail + n as u64, Ordering::Release);
        if n < src.len() {
            self.cached_head = self.inner.head.0.load(Ordering::Acquire);
        }
        n
    }

    /// Samples currently buffered (approximate: may lag the consumer slightly).
    pub(crate) fn len(&self) -> u64 {
        let tail = self.inner.tail.0.load(Ordering::Relaxed);
        let head = self.inner.head.0.load(Ordering::Acquire);
        tail.wrapping_sub(head)
    }

    /// Request that the consumer discard all currently buffered audio. Returns
    /// the new generation. Follow with [`RingProducer::wait_reset_acked`] and
    /// [`RingProducer::refresh`].
    pub(crate) fn request_reset(&mut self) -> u64 {
        self.inner.generation.0.fetch_add(1, Ordering::Release) + 1
    }

    /// Wait until the consumer has observed the reset generation `target`
    /// (i.e. finished discarding). Waiting on the ack — not on emptiness — is
    /// essential: a buffer that is already empty at reset time would let the
    /// producer proceed immediately, and the consumer would then discard the
    /// *new* audio on its next wake. Returns `true` if acked, `false` on
    /// timeout (safe to proceed regardless: the consumer discards whatever it
    /// finds, which merely costs one period of audio).
    pub(crate) fn wait_reset_acked(&self, target: u64, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.inner.acked_generation.0.load(Ordering::Acquire) >= target {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Refresh the cached consumer cursor after a reset (the consumer may have
    /// fast-forwarded it past our cached value).
    pub(crate) fn refresh(&mut self) {
        self.cached_head = self.inner.head.0.load(Ordering::Acquire);
    }
}

impl RingConsumer {
    /// Bring the consumer up to date with any reset request. If the generation
    /// changed, discard everything currently buffered (O(1)) and return the number
    /// of discarded samples. The caller must account them into its played-frames
    /// counter to keep the producer/consumer frame accounting FIFO-aligned.
    pub(crate) fn sync_generation(&mut self) -> usize {
        let generation = self.inner.generation.0.load(Ordering::Acquire);
        if generation == self.cached_gen {
            return 0;
        }
        self.cached_gen = generation;
        let tail = self.inner.tail.0.load(Ordering::Acquire);
        let head = self.inner.head.0.load(Ordering::Relaxed);
        let discarded = tail.wrapping_sub(head) as usize;
        // Jump the read cursor to the write cursor: everything buffered is dropped.
        self.inner.head.0.store(tail, Ordering::Release);
        self.cached_tail = tail;
        // Acknowledge so the producer knows it may write post-reset audio.
        self.inner
            .acked_generation
            .0
            .store(generation, Ordering::Release);
        discarded
    }

    /// Pop up to `dst.len()` samples of real audio. `dst.len()` must be a multiple
    /// of the channel count. Returns how many samples were filled.
    pub(crate) fn pop(&mut self, dst: &mut [f32]) -> usize {
        let mut total = self.pop_inner(dst);
        if total < dst.len() {
            // The cached producer cursor may be stale: refresh and retry once
            // for the remainder.
            self.cached_tail = self.inner.tail.0.load(Ordering::Acquire);
            total += self.pop_inner(&mut dst[total..]);
        }
        total
    }

    fn pop_inner(&mut self, dst: &mut [f32]) -> usize {
        if dst.is_empty() {
            return 0;
        }
        // SAFETY (see Inner): the consumer only reads cells between `head` and
        // `tail`; no producer write touches that range.
        let buf = unsafe { &*self.inner.buf.get() };
        let cap = buf.len();
        let head = self.inner.head.0.load(Ordering::Relaxed);
        let tail = self.cached_tail;
        let avail = tail.wrapping_sub(head) as usize;
        let n = avail.min(dst.len());
        if n == 0 {
            return 0;
        }
        let pos = (head & self.inner.mask) as usize;
        let first = n.min(cap - pos);
        dst[..first].copy_from_slice(&buf[pos..pos + first]);
        let second = n - first;
        if second > 0 {
            dst[first..n].copy_from_slice(&buf[..second]);
        }
        self.inner.head.0.store(head + n as u64, Ordering::Release);
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(n: usize) -> Vec<f32> {
        (0..n).map(|i| i as f32).collect()
    }

    #[test]
    fn push_pop_roundtrip() {
        let (mut prod, mut cons) = ring_buffer(8, 2);
        assert_eq!(prod.push(&frames(16)), 16);
        let mut out = vec![0.0; 8];
        assert_eq!(cons.pop(&mut out), 8);
        assert_eq!(out, frames(8));
        assert_eq!(cons.pop(&mut out), 8);
        assert_eq!(out, (8..16).map(|i| i as f32).collect::<Vec<_>>());
        assert_eq!(cons.pop(&mut out), 0);
    }

    #[test]
    fn wraps_around() {
        let (mut prod, mut cons) = ring_buffer(4, 2); // cap = 8 samples
        let mut out = [0.0; 4];
        for round in 0..10u32 {
            let data: Vec<f32> = (0..4).map(|i| (round * 4 + i) as f32).collect();
            // Push/pop 4 samples total per round. A call may return 0 once while
            // it refreshes its cached peer cursor (lazy refresh); callers retry.
            let mut src = data.clone();
            let mut popped = 0usize;
            while !src.is_empty() || popped < 4 {
                let n = prod.push(&src);
                src.drain(..n);
                popped += cons.pop(&mut out[popped..]);
            }
            assert_eq!(&out[..4], &data[..]);
        }
    }

    #[test]
    fn push_reports_partial_when_full() {
        let (mut prod, mut cons) = ring_buffer(2, 2); // cap = 4 samples
        assert_eq!(prod.push(&frames(4)), 4);
        assert_eq!(prod.push(&frames(4)), 0); // full (also refreshes cache)
        assert_eq!(prod.push(&frames(4)), 0); // still full
        let mut out = vec![0.0; 4];
        assert_eq!(cons.pop(&mut out), 4);
        // First push after the pops may lazily refresh the cached head...
        let pushed = prod.push(&frames(4));
        assert!(pushed == 0 || pushed == 4);
        // ...and a retry always succeeds now that there is space.
        let mut src = frames(4);
        while !src.is_empty() {
            let n = prod.push(&src);
            src.drain(..n);
        }
        assert_eq!(cons.pop(&mut out), 4);
    }

    #[test]
    fn generation_reset_discards() {
        let (mut prod, mut cons) = ring_buffer(8, 2);
        prod.push(&frames(16));
        let target = prod.request_reset();
        // Not acked until the consumer syncs.
        assert!(!prod.wait_reset_acked(target, Duration::from_millis(20)));
        let discarded = cons.sync_generation();
        assert_eq!(discarded, 16);
        assert!(prod.wait_reset_acked(target, Duration::from_millis(100)));
        let mut out = vec![0.0; 4];
        assert_eq!(cons.pop(&mut out), 0);
        prod.refresh();
        // Post-reset audio is not discarded.
        assert_eq!(prod.push(&frames(4)), 4);
        assert_eq!(cons.pop(&mut out), 4);
    }

    #[test]
    fn reset_on_empty_ring_keeps_post_reset_audio() {
        // Regression: an empty ring at reset time must still be acked before
        // new audio is pushed, or the consumer discards the *new* audio.
        let (mut prod, mut cons) = ring_buffer(8, 2);
        let target = prod.request_reset();
        // The consumer observes the reset while the ring is empty: nothing is
        // discarded, and the producer learns it may push post-reset audio.
        assert_eq!(cons.sync_generation(), 0);
        assert!(prod.wait_reset_acked(target, Duration::from_millis(100)));
        prod.refresh();
        // Post-reset audio therefore survives later syncs.
        assert_eq!(prod.push(&frames(8)), 8);
        assert_eq!(cons.sync_generation(), 0);
        let mut out = vec![0.0; 8];
        assert_eq!(cons.pop(&mut out), 8);
        assert_eq!(out, frames(8));
    }

    #[test]
    fn frame_granularity_preserved_when_almost_full() {
        // cap 8 samples = 4 stereo frames; push 3 frames then 2 frames -> second push partial
        let (mut prod, mut cons) = ring_buffer(4, 2);
        assert_eq!(prod.push(&frames(6)), 6);
        assert_eq!(prod.push(&frames(4)), 2); // only 1 frame fits (a whole frame)
        let mut out = vec![0.0; 8];
        assert_eq!(cons.pop(&mut out), 8);
        // Stream content: frames(6) followed by the first frame of frames(4).
        assert_eq!(out, vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 0.0, 1.0]);
        assert_eq!(cons.pop(&mut out), 0);
    }
}
