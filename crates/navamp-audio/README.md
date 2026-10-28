# navamp-audio

NavAmp's audio engine (M1): **symphonia decode → DSP → lock-free ring buffer → cpal output**, with a gapless play queue.

```rust
use navamp_audio::{Command, EngineHandle, TrackSource};

let engine = EngineHandle::start()?;              // opens the default output device
let events = engine.subscribe();                  // crossbeam Receiver<Event>
engine.send(Command::Play(TrackSource::File("a.flac".into())))?;
engine.send(Command::EnqueueNext(TrackSource::File("b.flac".into())))?; // gapless
engine.send(Command::SetVolume(0.8))?;            // ramped in the callback, no clicks
```

## Thread model

```text
UI / IPC thread                 producer thread ("navamp-audio-producer")     audio callback (device RT thread)
────────────────                ─────────────────────────────────────────    ────────────────────────────────
EngineHandle                    Producer                                     Callback
 ├─ Command ──crossbeam────▶    ├─ PcmSource (symphonia FormatReader+Decoder)  ├─ RingConsumer   (lock-free)
 ├─ atomics ◀─……Relaxed……▶     ├─ channel convert → rubato resample           ├─ DspChain       (preamp 1.0)
 └─ Receiver(Event) ◀──────     ├─ RingProducer (lock-free SPSC)               ├─ VolumeRamp     (de-zipper)
                                └─ Events broadcast ────────────────────────▶  └─ atomics: status/volume/played
```

* **One** decoder/producer thread owns all symphonia readers (current + one pre-opened next track).
* The **cpal callback** only touches the ring buffer, atomics and fixed per-block DSP state — it never allocates, locks, or blocks. The only `unsafe` in the crate is the SPSC ring's sample storage, with the disjoint-access invariant documented in `src/ring.rs`.
* **Position** is a samples-played `AtomicU64` on the consumer side; the producer keeps a FIFO-aligned pushed-frames counter and marks track boundaries ("sessions") on that shared timeline, so `position_secs()` is exact without the UI ever racing the audio path. Status is an `AtomicU8`, volume an f32 bit-cast into an `AtomicU32`.

## Key policies

* **Sample rates**: on `Play` the engine tries to open the device at the *source* rate (stereo) so most playback is bit-exact; if the device refuses, it keeps the running stream and resamples on the **decode thread** (rubato FFT, fixed chunks). Gapless transitions never re-open the device — a rate-mismatched next track is resampled instead.
* **Gapless**: `EnqueueNext` pre-opens the next file; decoded output continues in the same ring buffer — zero inserted silence. `TrackFinished`/`TrackStarted` fire producer-side when the swap is *scheduled* (leads the audible transition by ≤ ring latency, typically ≪100 ms). `EndOfQueue` fires when the last frame was actually consumed.
* **Pause/Resume**: the device stream keeps running and emits silence; the position freezes and buffered audio survives. **Stop** clears queue + buffers. **Seek** is sample-accurate where the container allows (WAV/FLAC), resets the ring via a lock-free generation handshake, and works mid-track *and* while paused (the decoder stays resident until the queue finishes).
* **Volume**: applied in the callback with a 30 ms linear ramp (no clicks/zipper). The **DSP insertion point** for M6 — replay gain → preamp → 10-band biquad EQ — is the `DspChain` call between ring and volume (`src/dsp.rs`); preamp placeholder is 1.0.
* **Errors**: a mid-track decode error emits `Event::Error` and skips to the next track or stops gracefully. Nothing in the audio path panics.

## Formats

MP3, FLAC, Ogg Vorbis, AAC/ALAC (M4A), WAV — probed by content (symphonia 0.5, gapless/encoder-delay trimming enabled).

> **Opus caveat**: symphonia 0.5/0.6 has no released Opus decoder (no `opus` feature exists upstream). The Ogg *container* is enabled, but `.opus` files will surface an `Event::Error`. Options for M6: the young `symphonia-adapter-opus-rs` crate, or an ffmpeg sidecar.

## Output abstraction

`AudioSink` abstracts the device: `CpalSink` (production, default output device, F32 stereo) and `TestSink` (cloneable, synchronous `pull()` driven by tests, **paced to real time** like a device, refuses configurations other than the one it was built with — which is how the resample path is tested deterministically).

## Testing

```sh
cargo test -p navamp-audio     # 6 ring unit tests + 9 hardware-free engine tests + doc test
cargo clippy -p navamp-audio --all-targets
cargo fmt
```

Integration tests generate PCM16 sine WAV fixtures in-test and compare the engine output **sample-exactly** against the quantized sine: gapless A→B concatenation, pause/resume splice, post-seek waveform, volume-ramp continuity, plus 48 kHz→44.1 kHz resampling (signal span ±1%), mono→stereo, stop semantics and error handling.

## Example

```sh
cargo run -p navamp-audio --example play_file -- /path/to/song.mp3   # Ctrl-C exits cleanly
```
