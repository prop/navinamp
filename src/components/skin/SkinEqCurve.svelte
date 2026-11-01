<script lang="ts">
  /**
   * Live EQ response drawn over a skin's `eq.graph` layer (classic `.wsz`
   * skins and the fallback). Same approximate Gaussian model as the HTML
   * EqPanel: each band contributes a ~1-octave bell in log2-frequency space.
   * The preamp is deliberately not drawn — it is not part of a preset and
   * would just shift the whole line.
   */
  import type { ResolvedObject } from "../../lib/skin/resolver";
  import type { EqLive } from "../../lib/audio/eq";

  interface Props {
    obj: ResolvedObject;
    eq: EqLive | null;
  }

  let { obj, eq }: Props = $props();

  // Classic Winamp EQ center frequencies (Hz), matching the sliders.
  const BAND_HZ = [60, 170, 310, 600, 1000, 3000, 6000, 12000, 14000, 16000];
  const DB_MIN = -12;
  const DB_MAX = 12;
  const DB_RANGE = DB_MAX - DB_MIN;

  let canvas = $state<HTMLCanvasElement | null>(null);

  function responseDb(f: number, bands: number[]): number {
    let sum = 0;
    for (let i = 0; i < BAND_HZ.length; i++) {
      const oct = Math.log2(f / BAND_HZ[i]);
      sum += (bands[i] ?? 0) * Math.exp(-(oct * oct) / 0.4);
    }
    return sum;
  }

  $effect(() => {
    const bands = eq?.bandsDb ?? [];
    for (const db of bands) void db; // track every band

    const ctx = canvas?.getContext("2d");
    if (!ctx) return;
    const w = obj.w ?? 0;
    const h = obj.h ?? 0;
    if (w <= 0 || h <= 0) return;

    ctx.clearRect(0, 0, w, h);
    const mid = h / 2;
    const yFor = (db: number) =>
      mid - (Math.max(DB_MIN, Math.min(DB_MAX, db)) / DB_RANGE) * (h - 2);

    // 0 dB midline.
    ctx.strokeStyle = "rgba(100, 100, 112, 0.85)";
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(0, Math.round(mid) + 0.5);
    ctx.lineTo(w, Math.round(mid) + 0.5);
    ctx.stroke();

    // Response curve, 20 Hz → 20 kHz on a log-x axis.
    ctx.strokeStyle = "#3ccb4e";
    ctx.beginPath();
    const N = Math.max(48, w);
    for (let i = 0; i <= N; i++) {
      const f = 20 * Math.pow(1000, i / N);
      const x = Math.round((i / N) * (w - 1)) + 0.5;
      const y = Math.round(yFor(responseDb(f, bands))) + 0.5;
      if (i === 0) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    }
    ctx.stroke();
  });
</script>

<canvas
  bind:this={canvas}
  width={obj.w ?? 0}
  height={obj.h ?? 0}
  style="left:{obj.x}px; top:{obj.y}px; width:{obj.w ?? 0}px; height:{obj.h ?? 0}px;"
  aria-hidden="true"
></canvas>

<style>
  canvas {
    position: absolute;
    image-rendering: pixelated;
    pointer-events: none;
    user-select: none;
  }
</style>
