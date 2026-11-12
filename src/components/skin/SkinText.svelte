<script lang="ts">
  /**
   * `<text>`: live text objects. Two rendering paths; the smooth HTML path
   * is the default for every skin (see `CLASSIC_BITMAP_TEXT` below) because
   * the classic strips are the awkward part at fractional UI scales.
   *
   *  · BITMAP GLYPHS (opt-in; classic `.wsz` skins) — when the object carries
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
   *  · HTML TEXT (default for all skins) — font-styled spans;
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

  /** Classic normal-mode time: digits sit at the strip's fixed cell offsets. */
  const classicNumberTime = $derived(
    o?.display === "time" && charset === "classic-numbers",
  );
  /** Classic windowshade clock: the 5×6 text strip, including its colon cell. */
  const classicShadeTime = $derived(o?.display === "time" && charset === "classic-text");
  /** HTML text for the time: padded for the classic cells, plain m:ss otherwise. */
  const timeText = $derived(classicShadeTime ? glyphTime : content);
  /** Classic bitrate/kHz digit cells are narrower than the default 9px span. */
  const classicReadout = $derived(
    charset != null && (o?.display === "bitrate" || o?.display === "khz"),
  );
  /** Fallback x offsets of the four classic time digits (numbers.bmp cells). */
  const CLASSIC_DIGIT_OFFSETS = [12, 24, 42, 54];
  /**
   * Silkscreen's line box is ~1.3× its font size: 10px measures 13px tall,
   * exactly the classic numbers cell. Scale that ratio for skins with other
   * cell heights — using the cell height as the font size overflows it.
   */
  const classicDigitFont = $derived(
    Math.max(1, Math.round(((bmSpec?.cellH ?? 13) * 10) / 13)),
  );

  /**
   * Unicode typography → classic charmap equivalents. text.bmp is a 1997
   * ASCII strip (dashes, straight quotes only), so a single en dash or curly
   * quote otherwise pushes the whole ticker onto the HTML fallback font.
   *
   * Entries are grouped by class (dashes, quotes/primes, symbols, spaces) so
   * the table stays auditable against the charmap; each target must be a real
   * cell (see `CLASSIC_TEXT_SPEC.charmap`), not just plausible ASCII.
   */
  const CLASSIC_GLYPH_SUBSTITUTIONS: Record<string, string> = {
    "\u2010": "-", // hyphen
    "\u2011": "-", // non-breaking hyphen
    "\u2012": "-", // figure dash
    "\u2013": "-", // en dash
    "\u2014": "-", // em dash
    "\u2015": "-", // horizontal bar
    "\u2018": "'", // left single quote
    "\u2019": "'", // right single quote
    "\u201a": "'", // single low quote
    "\u201b": "'",
    "\u201c": '"', // left double quote
    "\u201d": '"', // right double quote
    "\u201e": '"',
    "\u201f": '"',
    "\u2032": "'", // prime
    "\u2033": '"', // double prime
    "\u00ab": '"', // left guillemet
    "\u00bb": '"', // right guillemet
    "\u2022": "*", // bullet
    "\u00b7": "*", // middle dot (the usual metadata separator)
    "\u00d7": "x", // multiplication sign
    "\u00a0": " ", // non-breaking space
    "\u2009": " ", // thin space
    "\u200a": " ",
    "\u202f": " ", // narrow no-break space
  };

  /**
   * Downgrade Unicode typography to the classic charmap's ASCII cells before
   * glyph rendering, so fancy punctuation keeps the pixel font. Scripts with
   * no mapping (Cyrillic, CJK, emoji) still fall back to HTML below.
   */
  function classicGlyphText(text: string): string {
    let out = "";
    for (const ch of text) out += CLASSIC_GLYPH_SUBSTITUTIONS[ch] ?? ch;
    return out;
  }

  /** What the bitmap path actually draws (typography downgraded). */
  const glyphText = $derived(classicGlyphText(glyphContent));

  /**
   * Text the glyph renderer actually blits: the numbers charmap has no
   * colon cell (the classic time skips it), so coverage is checked against
   * the drawn characters.
   */
  const coverageText = $derived(
    o?.display === "time" && charset !== "classic-text"
      ? glyphText.replace(":", "")
      : glyphText,
  );

  /**
   * Whether the bitmap font can represent the text: every non-space char
   * needs a cell (after foldLower/aliases). The classic text.bmp has no
   * Cyrillic (or CJK/emoji) glyphs, so repaired UTF-8 titles would render as
   * blanks — those objects fall back to the HTML path instead.
   */
  const glyphsFit = $derived.by(() => {
    const f = font;
    if (!f) return false;
    for (const ch of coverageText) {
      if (ch === " " || ch === "\u00a0") continue;
      if (!f.cell(ch)) return false;
    }
    return true;
  });

  /**
   * Classic skins *can* draw text with the skin's own bitmap strips
   * (text.bmp/numbers.bmp). The renderer now prefers the smooth HTML path for
   * every text object: at fractional UI scales the bitmap font is the
   * awkward, phase-shifting part, and the system font keeps the skin's glyph
   * colour while staying legible. Flip this to `true` to restore the classic
   * bitmap text — the glyph path below is kept intact for that.
   */
  const CLASSIC_BITMAP_TEXT = false;

  const useGlyphs = $derived(CLASSIC_BITMAP_TEXT && font != null && glyphsFit);

  // HTML-path colour for a bitmap skin. Don't sample unless the glyph path is
  // actually unavailable (sampling is a real getImageData, and must not be
  // pinned by an early read). While a declared strip is still loading or
  // failed, use a neutral colour instead of the generic green; time readouts
  // have their own green default and previously kept it forever on a missing
  // numbers.bmp.
  const fallbackColor = $derived.by(() => {
    if (useGlyphs) return null;
    if (font != null) return font.fallbackColor();
    return o?.params?.["font"] ? "#c8c8c8" : null;
  });

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
    const text = glyphText; // redraw when the live value changes

    const ctx = el.getContext("2d");
    if (!ctx) return;
    ctx.imageSmoothingEnabled = false;

    // Time display: fixed digit cells; hidden while stopped, blink paused.
    if (o.display === "time") {
      if (status === "stopped") {
        ctx.clearRect(0, 0, w, h);
        return;
      }
      // The windowshade clock uses text.bmp's 5×6 glyphs, which include a
      // colon cell (numbers.bmp has none — its colon is baked into main.bmp).
      // Clip to the object rect so a hostile/odd skin cannot overdraw the
      // strip (the glyph run can exceed the box on long times).
      if (charset === "classic-text") {
        ctx.clearRect(0, 0, w, h);
        ctx.save();
        ctx.beginPath();
        ctx.rect(0, 0, w, h);
        ctx.clip();
        f.drawText(ctx, text, 0, 0);
        ctx.restore();
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
    // Classic bitmap skins: when a Cyrillic/CJK title cannot use the strip,
    // keep the HTML fallback in the strip's own glyph colour instead of the
    // generic green. Non-bitmap skins leave `font` null and keep CSS green.
    return [
      `left:${obj.x}px`,
      `top:${obj.y}px`,
      `width:${obj.w ?? 0}px`,
      `height:${obj.h ?? 0}px`,
      p["color"] ? `color:${p["color"]}` : fallbackColor ? `color:${fallbackColor}` : "",
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
    if (!marquee || useGlyphs || !box || !probe) {
      overflow = false;
      return;
    }
    overflow = probe.offsetWidth > box.clientWidth + 1;
    offset = 0;
    if (rollEl) rollEl.style.transform = "translateX(0)";
  });

  // Scroll loop — only while a songticker actually overflows.
  $effect(() => {
    if (!marquee || useGlyphs || !overflow) return;
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
  {#if useGlyphs}
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
    <!-- Smooth HTML path (the default for every skin). Classic time digits
         are placed at the strip's own cell offsets so they line up with the
         dim "88:88" ghost baked into main.bmp; the colon stays the ghost's
         (numbers.bmp has no colon cell). -->
    <div
      class="txt"
      class:time={o.display === "time"}
      class:readout={classicReadout}
      class:shade-time={classicShadeTime}
      bind:this={boxEl}
      {style}>
      {#if classicNumberTime}
        <span class="ctime" class:blink={status === "paused"}>
          {#if status !== "stopped"}
            {#each [...glyphTime.replace(":", "")] as ch, i}
              <span
                class="cdigit"
                style="left:{digitOffsets?.[i] ?? CLASSIC_DIGIT_OFFSETS[i] ?? 0}px;
                  width:{bmSpec?.cellW ?? 9}px;font-size:{classicDigitFont}px;
                  line-height:{bmSpec?.cellH ?? 13}px"
                >{ch}</span>
            {/each}
          {/if}
        </span>
      {:else}
        <span class="probe" aria-hidden="true" bind:this={probeEl}>{timeText}</span>
        {#if marquee && overflow}
          <span class="roll" bind:this={rollEl}>
            <span class="unit" bind:this={unitEl}>{timeText}{SEP}</span>
            <span class="unit" aria-hidden="true">{timeText}{SEP}</span>
          </span>
        {:else}
          <span class="unit">{timeText}</span>
        {/if}
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

  /* Windowshade clock: 7px is the largest size that fits the 25×8 strip.
     The font's line box exceeds 8px by 1px, but its ink is 5px, so the
     overflow is empty em-box (no glyph is clipped) — leave no extra slack. */
  .txt.shade-time {
    font-size: 7px;
    letter-spacing: 0;
  }

  /* Classic bitrate/kHz wells are 5px per digit; the 8px face measured
     ~5.7px/digit (17px for three) and overflowed the 15px well — 7px fits. */
  .txt.readout {
    font-size: 7px;
  }

  /* Classic normal-mode time: one digit per numbers.bmp cell. */
  .ctime {
    position: absolute;
    inset: 0;
  }

  .cdigit {
    position: absolute;
    top: 0;
    height: 100%;
    display: flex;
    align-items: center;
    justify-content: center;
    font-family: "Silkscreen", ui-monospace, "SF Mono", Menlo, Consolas, monospace;
  }

  .ctime.blink {
    animation: blink 2s step-start 1s infinite;
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
