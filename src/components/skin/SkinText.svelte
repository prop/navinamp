<script lang="ts">
  /**
   * `<text>`: live text objects. Two rendering paths, picked per object:
   *
   *  · BITMAP GLYPHS (classic `.wsz` skins) — when the object carries
   *    `params["font"]` (a `<bitmapfont>` id) whose glyph strip was
   *    materialized as `images["{font}.image"]`, text is blitted from the
   *    strip (text.bmp/numbers.bmp) exactly like Winamp 2: opaque, crisp,
   *    pixel-perfect, single-case. `params["charset"]` names the cell map
   *    ("classic-text" | "classic-numbers"; see lib/skin/bmfont.ts).
   *      – display="songticker": marquee in classic 5px char steps every
   *        220 ms with a "  ***  " separator when the title overflows its
   *        rect (Winamp's chunky scroll — not a smooth marquee).
   *      – display="time": digits from numbers.bmp at the fixed cell
   *        offsets in `params["digitoffsets"]` (the ":" colon is baked into
   *        main.bmp, so it is skipped); hidden while stopped, blinking while
   *        paused (classic behavior).
   *      – params["display"]="bitrate"/"khz": the digit cells next to the
   *        "kbps"/"khz" labels that are baked into main.bmp. Values come
   *        from the engine's `audio_state` metadata (average kbps, source
   *        kHz); the cells stay empty when idle or when the engine does not
   *        report metadata.
   *  · HTML TEXT (`.wal` skins / the fallback skin) — font-styled spans;
   *    the Wasabi `display` attribute decides the CONTENT and takes
   *    precedence over `params.text` — the fallback skin sets text=" " on
   *    its `display="time"` object purely as a ghost-suppressor (a dim
   *    "88:88" bitmap shows through), so checking params.text first would
   *    render a blank where the time belongs. Precedence: display →
   *    params.text → "" (ids are never rendered — that was the garbage the
   *    classic-skin acceptance run caught).
   *
   *  · display="songticker" — "Artist - Title" of the current queue item.
   *  · display="time" — live m:ss from the caller's 10 Hz audio_state poll.
   *  · display="status" — PLAYING / PAUSED / STOPPED.
   */
  import type { ResolvedObject } from "../../lib/skin/resolver";
  import type { PlaybackStatus } from "../../lib/skin/wasabi";
  import { loadBmFont, specForCharset, type BmFont } from "../../lib/skin/bmfont";
  import { formatTime } from "../../lib/player/queue.svelte";

  interface Props {
    obj: ResolvedObject;
    status: PlaybackStatus;
    /** Playback position in seconds (the App's polled value). */
    position: number;
    /** Songticker content ("Artist - Title"), fed from queue.currentItem. */
    ticker: string;
    /** Average bitrate (kbps) of the current track, from audio_state. */
    bitrateKbps?: number | null;
    /** Source sample rate (Hz) of the current track, from audio_state. */
    sampleRateHz?: number | null;
    /** bitmap id -> data URL (the layout's image table). */
    images?: Record<string, string>;
  }

  let { obj, status, position, ticker, bitrateKbps = null, sampleRateHz = null, images }: Props =
    $props();

  const o = $derived(obj.object.kind === "text" ? obj.object : null);
  const params = $derived(o?.params ?? {});
  const marquee = $derived(o?.display === "songticker");

  // ── bitmap-glyph path setup ────────────────────────────────────────────────

  /** params["charset"], or the id-derived guess for payloads without it. */
  const charset = $derived.by(() => {
    const named = params["charset"];
    if (named) return named;
    const font = params["font"] ?? "";
    if (font.endsWith("font.numbers")) return "classic-numbers";
    if (font.endsWith("font.text")) return "classic-text";
    return undefined;
  });

  /** Geometry overrides from params (fallback-skin 2x strip cells etc.). */
  const bmSpec = $derived.by(() => {
    const base = specForCharset(charset);
    if (!base) return null;
    const num = (k: string): number | undefined => {
      const v = parseInt(params[k] ?? "", 10);
      return Number.isFinite(v) && v > 0 ? v : undefined;
    };
    const cellW = num("glyphw");
    const cellH = num("glyphh");
    if (cellW === undefined && cellH === undefined) return base;
    return {
      ...base,
      cellW: cellW ?? base.cellW,
      cellH: cellH ?? base.cellH,
      strideX: num("stridex") ?? cellW ?? base.strideX,
      strideY: num("stridey") ?? cellH ?? base.strideY,
      cols: num("cols") ?? base.cols,
      advance: num("advance") ?? cellW ?? base.advance,
    };
  });

  const stripUrl = $derived.by(() => {
    const font = params["font"];
    if (!font) return undefined;
    // Primary keying: the pipeline materializes `<bitmapfont>` strips as
    // `images["<font>.image"]`. The direct `<font>` key is a defensive
    // fallback for payload variants.
    return images?.[`${font}.image`] ?? images?.[font];
  });

  let font = $state<BmFont | null>(null);
  let stripWarned = false;
  $effect(() => {
    const url = stripUrl;
    const spec = bmSpec;
    font = null;
    if (!url || !spec) {
      // Diagnostics: WHY did a display="time" object fall back to HTML?
      // (The classic time should render via numbers.bmp glyphs — if you
      // see thin HTML digits, this warning names the broken link.)
      if (!stripWarned && o?.display === "time") {
        stripWarned = true;
        console.warn(
          `[navinamp] time fell back to HTML — font:${params["font"] ?? "∅"} ` +
            `strip:${url ?? "∅"} charset:${charset ?? "∅"} ` +
            `images-keys:${images ? Object.keys(images).length : "no-images"}`,
        );
      }
      return;
    }
    stripWarned = false;
    let alive = true;
    loadBmFont(url, spec)
      .then((f) => {
        if (alive) font = f;
      })
      .catch(() => undefined); // undecodable strip → HTML fallback
    return () => {
      alive = false;
    };
  });

  /** params["digitoffsets"]: x positions of the digit cells inside the rect. */
  const digitOffsets = $derived.by(() => {
    const raw = params["digitoffsets"];
    if (!raw) return null;
    const list = raw
      .split(",")
      .map((v) => parseInt(v, 10))
      .filter((n) => Number.isFinite(n) && n >= 0);
    return list.length > 0 ? list : null;
  });

  // Content per display kind (the bitmap and HTML paths share it).
  const content = $derived.by(() => {
    if (!o) return "";
    switch (o.display) {
      case "songticker":
        return ticker;
      case "time":
        return formatTime(Math.max(0, position));
      case "status":
        return status.toUpperCase();
      // Classic synthesized readouts: digit cells beside the "kbps"/"khz"
      // labels baked into main.bmp. Emptied when the engine reports no
      // metadata (stopped, or a source without a known duration).
      case "bitrate":
        return bitrateKbps == null ? "" : String(Math.round(bitrateKbps));
      case "khz":
        return sampleRateHz == null ? "" : String(Math.round(sampleRateHz / 1000));
      default:
        break;
    }
    // Classic synthesized readouts via params (Status-family payloads and
    // hand-built objects; Text params are serde-flattened over IPC, so this
    // only matters for in-window constructions).
    switch (params["display"]) {
      case "bitrate":
        return bitrateKbps == null ? "" : String(Math.round(bitrateKbps));
      case "khz":
        return sampleRateHz == null ? "" : String(Math.round(sampleRateHz / 1000));
      default:
        return params["text"] ?? "";
    }
  });

  /**
   * Bitmap-path time text: always two minute digits ("05:23") — the classic
   * display has four fixed digit cells and no leading-zero truncation
   * (minutes clamp at 99, like the four-cell hardware display).
   */
  const glyphTime = $derived.by(() => {
    const t = Math.max(0, position);
    const m = Math.min(99, Math.floor(t / 60));
    const s = Math.floor(t % 60);
    return `${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
  });

  /** What the glyph path draws: padded time, otherwise the shared content. */
  const glyphContent = $derived((o?.display === "time" ? glyphTime : content));

  // ── bitmap-glyph canvas ────────────────────────────────────────────────────

  let canvasEl = $state<HTMLCanvasElement | null>(null);

  // Classic marquee: one char cell per 220 ms tick (Webamp's SEPARATOR).
  const TICKER_STEP_MS = 220;
  const TICKER_SEPARATOR = "  ***  ";

  $effect(() => {
    const f = font;
    const el = canvasEl;
    if (!f || !el || !o) return;
    const w = obj.w ?? 0;
    const h = obj.h ?? 0;
    const text = glyphContent; // redraw when the live value changes

    const ctx = el.getContext("2d");
    if (!ctx) return;
    ctx.imageSmoothingEnabled = false;

    // Time display: fixed digit cells; hidden while stopped, blink paused.
    if (o.display === "time") {
      if (status === "stopped") {
        ctx.clearRect(0, 0, w, h);
        return;
      }
      const digits = text.replace(":", "");
      const advance = f.spec.advance ?? f.spec.cellW;
      const offsets = digitOffsets ?? [...digits].map((_, i) => i * advance);
      ctx.clearRect(0, 0, w, h);
      [...digits].forEach((ch, i) => {
        f.drawText(ctx, ch, offsets[i] ?? i * advance, 0);
      });
      return;
    }

    // Songticker: classic chunky scroll once the title overflows the rect.
    if (marquee && f.measure(text) > w) {
      const unit = text + TICKER_SEPARATOR;
      const unitW = f.measure(unit);
      let offset = 0;
      const step = f.spec.advance ?? f.spec.cellW;
      const timer = setInterval(() => {
        offset = (offset + step) % unitW;
        ctx.clearRect(0, 0, w, h);
        // The unit is drawn twice so the wrap is seamless at any offset.
        f.drawText(ctx, unit, -offset, 0);
        f.drawText(ctx, unit, -offset + unitW, 0);
      }, TICKER_STEP_MS);
      return () => clearInterval(timer);
    }

    // Static glyphs, top-aligned (classic display wells are top-aligned).
    ctx.clearRect(0, 0, w, h);
    f.drawText(ctx, text, 0, 0);
  });

  // ── HTML path styling (unchanged for .wal / fallback skins) ────────────────

  const style = $derived.by(() => {
    const p = o?.params ?? {};
    return [
      `left:${obj.x}px`,
      `top:${obj.y}px`,
      `width:${obj.w ?? 0}px`,
      `height:${obj.h ?? 0}px`,
      p["color"] ? `color:${p["color"]}` : "",
      p["font"] && !stripUrl ? `font-family:${p["font"]}` : "",
      p["fontsize"] ? `font-size:${p["fontsize"]}px` : "",
      p["bold"] === "1" ? "font-weight:bold" : "",
    ]
      .filter(Boolean)
      .join(";");
  });

  // ── songticker marquee (HTML path) ─────────────────────────────────────────
  // A hidden probe span (same font, same text) measures the label without the
  // scroll markup, so "does it fit?" stays correct after the overflow switch.
  const SCROLL_PX_S = 40;
  const SEP = " • ";

  let boxEl = $state<HTMLElement | null>(null);
  let probeEl = $state<HTMLElement | null>(null);
  let rollEl = $state<HTMLElement | null>(null);
  let unitEl = $state<HTMLElement | null>(null);
  let overflow = $state(false);
  let offset = 0;

  // Re-measure whenever the label (or the box) changes; reset the scroll.
  $effect(() => {
    void content; // label drives the probe's width
    const box = boxEl;
    const probe = probeEl;
    if (!marquee || font || !box || !probe) {
      overflow = false;
      return;
    }
    overflow = probe.offsetWidth > box.clientWidth + 1;
    offset = 0;
    if (rollEl) rollEl.style.transform = "translateX(0)";
  });

  // Scroll loop — only while a songticker actually overflows.
  $effect(() => {
    if (!marquee || font || !overflow) return;
    let raf = 0;
    let last = performance.now();
    const step = (t: number) => {
      raf = requestAnimationFrame(step);
      const dt = t - last;
      last = t;
      const unit = unitEl?.offsetWidth ?? 0; // label + separator
      if (unit <= 0) return;
      offset = (offset + (dt / 1000) * SCROLL_PX_S) % unit;
      if (rollEl) rollEl.style.transform = `translateX(${-offset}px)`;
    };
    raf = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf);
  });
</script>

{#if o}
  {#if font}
    <!-- Bitmap glyphs: one canvas per object; the surface scales it with
         image-rendering: pixelated, so 2x mode stays crisp. -->
    <canvas
      bind:this={canvasEl}
      class="glyphs"
      class:time={o.display === "time"}
      class:blink={o.display === "time" && status === "paused"}
      width={obj.w ?? 0}
      height={obj.h ?? 0}
      {style}
    ></canvas>
  {:else}
    <div class="txt" class:time={o.display === "time"} bind:this={boxEl} {style}>
      <span class="probe" aria-hidden="true" bind:this={probeEl}>{content}</span>
      {#if marquee && overflow}
        <span class="roll" bind:this={rollEl}>
          <span class="unit" bind:this={unitEl}>{content}{SEP}</span>
          <span class="unit" aria-hidden="true">{content}{SEP}</span>
        </span>
      {:else}
        <span class="unit">{content}</span>
      {/if}
    </div>
  {/if}
{/if}

<style>
  .glyphs {
    position: absolute;
    image-rendering: pixelated;
    pointer-events: none;
    user-select: none;
  }

  /* Paused time display blinks, like the classic skin (2s period). */
  .glyphs.time.blink {
    animation: blink 2s step-start 1s infinite;
  }

  @keyframes blink {
    50% {
      visibility: hidden;
    }
  }

  .txt {
    position: absolute;
    overflow: hidden;
    white-space: nowrap;
    display: flex;
    align-items: center;
    color: #3ccb4e;
    font-family: Verdana, Tahoma, sans-serif;
    font-size: 9px;
    line-height: 1;
    user-select: none;
  }

  /* Big time readout: pixel font matched to the dim ghost's chunky digits,
     centered over it. Verdana here looked thin and misaligned. */
  .txt.time {
    justify-content: center;
    font-family: "Silkscreen", ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 10px; /* Silkscreen's grid at the ghost's 2x scale */
    letter-spacing: 1px;
    color: #66ff99;
  }

  .unit {
    white-space: pre;
  }

  .probe {
    position: absolute;
    visibility: hidden;
    white-space: pre;
  }

  .roll {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    white-space: pre;
    will-change: transform;
  }
</style>
