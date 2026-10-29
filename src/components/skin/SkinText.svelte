<script lang="ts">
  /**
   * `<text>`: live text objects (M6). The Wasabi `display` attribute decides
   * the CONTENT and takes precedence over `params.text` — the fallback skin
   * sets text=" " on its `display="time"` object purely as a ghost-suppressor
   * (a dim "88:88" bitmap shows through), so checking params.text first would
   * render a blank where the time belongs. Precedence: display → params.text
   * → id.
   *
   *  · display="songticker" — "Artist - Title" of the current queue item,
   *    marquee-scrolled at ~40 px/s with a " • " separator when it overflows
   *    its rect; static while it fits.
   *  · display="time" — live m:ss from the caller's 10 Hz audio_state poll.
   *  · display="status" — PLAYING / PAUSED / STOPPED.
   */
  import type { ResolvedObject } from "../../lib/skin/resolver";
  import type { PlaybackStatus } from "../../lib/skin/wasabi";
  import { formatTime } from "../../lib/player/queue.svelte";

  interface Props {
    obj: ResolvedObject;
    status: PlaybackStatus;
    /** Playback position in seconds (the App's polled value). */
    position: number;
    /** Songticker content ("Artist - Title"), fed from queue.currentItem. */
    ticker: string;
  }

  let { obj, status, position, ticker }: Props = $props();

  const o = $derived(obj.object.kind === "text" ? obj.object : null);
  const marquee = $derived(o?.display === "songticker");

  const content = $derived.by(() => {
    if (!o) return "";
    switch (o.display) {
      case "songticker":
        return ticker;
      case "time":
        return formatTime(Math.max(0, position));
      case "status":
        return status.toUpperCase();
      default:
        return o.params?.["text"] ?? o.id ?? "";
    }
  });

  const style = $derived.by(() => {
    const p = o?.params ?? {};
    return [
      `left:${obj.x}px`,
      `top:${obj.y}px`,
      `width:${obj.w ?? 0}px`,
      `height:${obj.h ?? 0}px`,
      p["color"] ? `color:${p["color"]}` : "",
      p["font"] ? `font-family:${p["font"]}` : "",
      p["fontsize"] ? `font-size:${p["fontsize"]}px` : "",
      p["bold"] === "1" ? "font-weight:bold" : "",
    ]
      .filter(Boolean)
      .join(";");
  });

  // ── songticker marquee ─────────────────────────────────────────────────────
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
    if (!marquee || !box || !probe) {
      overflow = false;
      return;
    }
    overflow = probe.offsetWidth > box.clientWidth + 1;
    offset = 0;
    if (rollEl) rollEl.style.transform = "translateX(0)";
  });

  // Scroll loop — only while a songticker actually overflows.
  $effect(() => {
    if (!marquee || !overflow) return;
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

<style>
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

  /* Big time readout: centered over its dim "88:88" ghost bitmap. */
  .txt.time {
    justify-content: center;
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
