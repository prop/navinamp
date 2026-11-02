<script lang="ts">
  /**
   * `<vis>`: live audio visualizer (M6). A canvas sized to the skin rect at
   * 1x and CSS-scaled with the surface (`image-rendering: pixelated`), so
   * bars stay crisp at 2x. Clicking cycles BARS → OSCILLOSCOPE → OFF; the
   * choice is shared by every vis and persisted (spectrum store).
   *
   * Data comes from the shared per-frame `audio_spectrum` poller: instances
   * subscribe while mounted and not OFF, and ask for data only while the
   * engine is playing. Paused freezes the last painted frame; stopped
   * decays the bars to zero client-side (no IPC for either).
   */
  import type { ResolvedObject } from "../../lib/skin/resolver";
  import type { PlaybackStatus } from "../../lib/skin/wasabi";
  import {
    cycleVisMode,
    subscribeSpectrum,
    visMode,
    type SpectrumSnapshot,
  } from "../../lib/audio/spectrum.svelte";

  interface Props {
    obj: ResolvedObject;
    status: PlaybackStatus;
  }

  let { obj, status }: Props = $props();

  const o = $derived(obj.object.kind === "vis" ? obj.object : null);
  const w = $derived(obj.w ?? 72);
  const h = $derived(obj.h ?? 16);

  let canvas = $state<HTMLCanvasElement | null>(null);

  /** Max bars (normal-mode density); narrow wells use fewer, chunkier bars. */
  const MAX_BARS = 32;
  /** ~2px per bar keeps the shade analyzer discrete instead of a 1px haze. */
  const bars = $derived(Math.max(16, Math.min(MAX_BARS, Math.floor(w / 2))));
  /** Bar fall per frame — the classic smooth falloff. */
  const BAR_FALL = 0.055;

  // Per-instance animation state (plain fields: only this canvas renders them).
  const levels = new Float32Array(MAX_BARS);
  const peaks = new Float32Array(MAX_BARS);

  /**
   * Classic `viscolor.txt` palette from the skin's vis object:
   * 0 background, 1 grid dots, 2..17 spectrum gradient (top→bottom),
   * 18..22 oscilloscope, 23 analyzer peaks. Absent → the fallback greens.
   */
  const visColors = $derived.by(() => {
    const raw = o?.params?.["viscolors"];
    if (!raw) return null;
    const colors = raw
      .split(";")
      .map((entry) => entry.split(",").map((n) => Number.parseInt(n, 10)))
      .filter((c) => c.length === 3 && c.every((n) => Number.isFinite(n)))
      .map(([r, g, b]) => `rgb(${r},${g},${b})`);
    return colors.length > 0 ? colors : null;
  });
  let wave: number[] = [];
  let live = false;
  /** Fully decayed + idle: nothing left to animate, skip canvas work. */
  let silent = true;

  function onFrame(frame: SpectrumSnapshot): void {
    if (visMode.mode === "off") return;
    if (status === "paused") return; // freeze: keep the last painted frame
    live = status === "playing" && frame.active;
    if (live) wave = frame.waveform;
    const peakFall = 2 / Math.max(1, h); // peaks drop ~2px/frame, classic feel
    let any = live;
    for (let i = 0; i < bars; i++) {
      const target = live ? (frame.spectrum[i] ?? 0) : 0; // stopped → decay to zero
      levels[i] = Math.max(target, levels[i] - BAR_FALL);
      peaks[i] = Math.max(levels[i], peaks[i] - peakFall);
      if (levels[i] > 0.004 || peaks[i] > 0.01) any = true;
    }
    if (!any) {
      // One final clear-draw after the decay, then zero per-frame work.
      if (!silent) {
        silent = true;
        draw(visMode.mode);
      }
      return;
    }
    silent = false;
    draw(visMode.mode);
  }

  function draw(mode: "bars" | "scope" | "off"): void {
    const ctx = canvas?.getContext("2d");
    if (!ctx) return;
    ctx.clearRect(0, 0, w, h);
    // Classic skins paint the well with viscolor 0; the fallback stays
    // transparent so the skin art shows through.
    if (visColors) {
      ctx.fillStyle = visColors[0] ?? "transparent";
      ctx.fillRect(0, 0, w, h);
    }
    if (mode === "bars") drawBars(ctx);
    else if (mode === "scope") drawScope(ctx);
    else drawGrid(ctx);
  }

  /** Classic gradient bars with decaying peak markers. */
  function drawBars(ctx: CanvasRenderingContext2D): void {
    const top = visColors?.[2] ?? "#8dffa8";
    const mid = visColors?.[9] ?? "#2fae4a";
    const bottom = visColors?.[17] ?? "#0a4a1c";
    const peak = visColors?.[23] ?? "#66ff99"; // fallback skin's peaks green
    const grad = ctx.createLinearGradient(0, h, 0, 0);
    grad.addColorStop(0, bottom);
    grad.addColorStop(0.55, mid);
    grad.addColorStop(1, top);
    const slot = w / bars;
    const bw = Math.max(1, Math.floor(slot) - 1); // 1px gap
    for (let i = 0; i < bars; i++) {
      const x = Math.round(i * slot);
      // Clamp to the canvas: a rounded last slot must never paint past `w`.
      const drawW = Math.min(x + bw, w) - x;
      if (drawW <= 0) continue;
      const bh = Math.round(levels[i] * (h - 1));
      if (bh > 0) {
        ctx.fillStyle = grad;
        ctx.fillRect(x, h - bh, drawW, bh);
      }
      if (peaks[i] > 0.01) {
        const py = Math.round(peaks[i] * (h - 1));
        ctx.fillStyle = peak;
        ctx.fillRect(x, Math.max(0, h - py - 2), drawW, 2);
      }
    }
  }

  /** 128-point waveform polyline in the skin's scope color; flat line when idle. */
  function drawScope(ctx: CanvasRenderingContext2D): void {
    ctx.strokeStyle = visColors?.[18] ?? "#3ccb4e";
    ctx.lineWidth = 1;
    ctx.beginPath();
    const n = Math.min(128, wave.length);
    if (live && n >= 2) {
      for (let i = 0; i < n; i++) {
        const x = (i / (n - 1)) * (w - 1);
        const y = h / 2 + (wave[i] ?? 0) * (h / 2 - 1);
        if (i === 0) ctx.moveTo(x, y);
        else ctx.lineTo(x, y);
      }
    } else {
      ctx.moveTo(0, h / 2);
      ctx.lineTo(w - 1, h / 2);
    }
    ctx.stroke();
  }

  /** OFF: dim grid only (classic viscolor 1 when the skin provides one). */
  function drawGrid(ctx: CanvasRenderingContext2D): void {
    ctx.strokeStyle = visColors?.[1] ?? "rgba(60, 203, 78, 0.16)";
    ctx.lineWidth = 1;
    ctx.beginPath();
    for (let y = 8; y < h; y += 8) {
      ctx.moveTo(0, y + 0.5);
      ctx.lineTo(w, y + 0.5);
    }
    for (let x = 16; x < w; x += 16) {
      ctx.moveTo(x + 0.5, 0);
      ctx.lineTo(x + 0.5, h);
    }
    ctx.stroke();
  }

  $effect(() => {
    const mode = visMode.mode;
    void w;
    void h; // re-init the canvas when the skin rect changes
    if (!canvas) return;
    if (mode === "off") {
      // Static grid: no subscription, zero poll. `draw` clears the last
      // painted frame (and applies the classic background when present).
      draw("off");
      return;
    }
    draw(mode);
    return subscribeSpectrum({
      onFrame,
      wantsData: () => status === "playing" && visMode.mode !== "off",
    });
  });
</script>

{#if o}
  <canvas
    bind:this={canvas}
    class="vis"
    width={w}
    height={h}
    style="left:{obj.x}px; top:{obj.y}px; width:{w}px; height:{h}px;"
    role="button"
    tabindex="0"
    title="visualizer: {visMode.mode} (click to cycle)"
    aria-label="visualizer, current mode {visMode.mode}"
    onclick={() => cycleVisMode()}
    onkeydown={(e) => {
      if (e.key !== "Enter" && e.key !== " ") return;
      e.preventDefault();
      cycleVisMode();
    }}
  ></canvas>
{/if}

<style>
  .vis {
    position: absolute;
    image-rendering: pixelated;
    cursor: pointer;
    outline: none;
  }

  .vis:focus-visible {
    outline: 1px solid #3ccb4e;
  }
</style>
