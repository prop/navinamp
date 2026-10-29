/**
 * Shared audio-spectrum store (M6): ONE `audio_spectrum` poll per animation
 * frame, shared by every `<vis>` instance.
 *
 * Lifecycle / zero-work guarantees:
 *  · The rAF loop runs only while at least one listener is subscribed; the
 *    last unsubscribe cancels it, so a skin without a visible vis never
 *    wakes the page or the IPC bridge.
 *  · `audio_spectrum` is invoked only when some listener's `wantsData()`
 *    is true (its status is `playing`) — paused (freeze) and stopped
 *    (client-side decay) cost zero IPC, as does an all-OFF page.
 *  · An in-flight guard keeps at most ONE outstanding invoke, so a frame
 *    that resolves slower than the display rate never queues a backlog.
 *  · A rejected command (backend without `audio_spectrum` yet) halts
 *    polling after a single console warn; a NEW subscription retries once,
 *    so a backend that lands later starts working without a reload.
 *
 * The vis mode (bars / scope / off) lives here too: it is shared state —
 * clicking any vis cycles every vis — and is persisted in localStorage.
 */

import { invoke } from "@tauri-apps/api/core";

/** One `audio_spectrum` snapshot (32 log bins 0..1, 128 samples -1..1). */
export interface SpectrumSnapshot {
  active: boolean;
  spectrum: number[];
  waveform: number[];
}

export type VisMode = "bars" | "scope" | "off";

const VIS_MODE_KEY = "navinamp.vis.mode";
const VIS_MODES: readonly VisMode[] = ["bars", "scope", "off"];

function loadMode(): VisMode {
  try {
    const v = localStorage.getItem(VIS_MODE_KEY);
    return v === "bars" || v === "scope" || v === "off" ? v : "bars";
  } catch {
    return "bars"; // storage unavailable (private mode &c.)
  }
}

/** Chosen vis mode, shared by all instances; mutate via cycleVisMode(). */
export const visMode = $state<{ mode: VisMode }>({ mode: loadMode() });

/** BARS → OSCILLOSCOPE → OFF → BARS; persists the choice. */
export function cycleVisMode(): VisMode {
  const next = VIS_MODES[(VIS_MODES.indexOf(visMode.mode) + 1) % VIS_MODES.length];
  visMode.mode = next;
  try {
    localStorage.setItem(VIS_MODE_KEY, next);
  } catch {
    // Storage unavailable — the choice just lives for this session.
  }
  return next;
}

export interface SpectrumListener {
  /** Called once per animation frame with the latest snapshot. */
  onFrame: (frame: SpectrumSnapshot) => void;
  /**
   * False while this listener cannot consume fresh data (paused, stopped,
   * OFF) — the poller skips `audio_spectrum` unless someone wants it.
   */
  wantsData: () => boolean;
}

const listeners = new Set<SpectrumListener>();
let rafId = 0;
let inFlight = false;
let failed = false;
let latest: SpectrumSnapshot = { active: false, spectrum: [], waveform: [] };

function tick(): void {
  rafId = requestAnimationFrame(tick);

  if (!inFlight && !failed) {
    let need = false;
    for (const l of listeners) {
      if (l.wantsData()) {
        need = true;
        break;
      }
    }
    if (need) {
      inFlight = true;
      invoke<SpectrumSnapshot>("audio_spectrum")
        .then((f) => {
          if (f) latest = f;
        })
        .catch((e) => {
          // Backend without the command (landing in parallel): stop asking.
          failed = true;
          console.warn(`[navinamp] audio_spectrum unavailable: ${e}`);
        })
        .finally(() => {
          inFlight = false;
        });
    }
  }

  // Every subscriber ticks each frame (decay/peaks are client-side), even
  // when the poll above was skipped — their onFrame decides what to skip.
  for (const l of listeners) l.onFrame(latest);
}

/**
 * Subscribe to the shared poller. Starts the rAF loop on the first
 * subscriber; returns the unsubscribe function (stops the loop when the
 * last one leaves).
 */
export function subscribeSpectrum(listener: SpectrumListener): () => void {
  listeners.add(listener);
  failed = false; // a fresh subscriber retries a command that failed before
  if (rafId === 0) rafId = requestAnimationFrame(tick);
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0 && rafId !== 0) {
      cancelAnimationFrame(rafId);
      rafId = 0;
    }
  };
}
