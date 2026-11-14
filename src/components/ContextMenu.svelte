<script module lang="ts">
  /** One row in an in-app context menu (see the instance script for docs). */
  export interface ContextMenuItem {
    id: string;
    label: string;
    danger?: boolean;
    disabled?: boolean;
    /** Draw a divider instead of a button (id/label are ignored). */
    separator?: boolean;
    /** When set (0–5), render the star picker row at this value. */
    rating?: number;
  }

  /** A non-interactive `label: value` detail row shown above the actions. */
  export interface ContextMenuInfoRow {
    label: string;
    value: string;
  }
</script>

<script lang="ts">
  /**
   * In-app context menu (replaces the webview's Reload/Inspect menu on rows).
   *
   * Portaled to the document body (see `portal`), so the menu escapes the
   * window's scaled/zoomed root and its text is rasterized at the final size
   * instead of being scaled as a bitmap. Positioned at the pointer's raw
   * viewport coordinates and clamped to the viewport. Closes on outside
   * click, Escape, scroll, resize or focus loss. Items are data-driven; a
   * `rating` item renders a 5-star picker and emits `rate:<n>` (or `rate:0`
   * when the current star is clicked again).
   */
  import { registerEscape } from "../lib/window/escape";
  import { portal } from "../lib/window/portal";

  interface Props {
    x: number;
    y: number;
    items: ContextMenuItem[];
    /** Optional track details (artist, album, quality, …) above the actions. */
    info?: ContextMenuInfoRow[];
    /**
     * UI scale of the app. The menu is portaled to the document body (real
     * viewport pixels), so `x`/`y` are raw client coordinates and the menu's
     * font/spacing are multiplied by this instead of living inside the
     * window's scaled/zoomed root. That keeps the text laid out at its final
     * size (crisp) while the menu still follows the UI scale, per #106.
     */
    scale?: number;
    onselect: (id: string) => void;
    onclose: () => void;
  }

  let { x, y, items, info = [], scale = 1, onselect, onclose }: Props = $props();

  let el = $state<HTMLDivElement | null>(null);
  // Initial placement in real viewport pixels; the effect below clamps it
  // after measuring. The component remounts per open, so capturing x/y once
  // is intended.
  // svelte-ignore state_referenced_locally
  let left = $state(x);
  // svelte-ignore state_referenced_locally
  let top = $state(y);

  // Clamp after measuring (the menu width/height depend on content; the
  // details block can arrive asynchronously after the metadata lookup).
  // The menu is body-level and size-scaled via `--s`, so these are real px.
  $effect(() => {
    void info;
    const node = el;
    if (!node) return;
    const w = node.offsetWidth;
    const h = node.offsetHeight;
    const margin = 4 * scale;
    const viewW = window.innerWidth;
    const viewH = window.innerHeight;
    left = Math.max(margin, Math.min(x, viewW - w - margin));
    top = Math.max(margin, Math.min(y, viewH - h - margin));
  });

  $effect(() => {
    const onDown = (e: PointerEvent) => {
      if (el && e.target instanceof Node && !el.contains(e.target)) onclose();
    };
    const unregisterEscape = registerEscape(onclose);
    const dismiss = () => onclose();
    // Scrolling the menu itself must not dismiss it. Only a scroll outside
    // (the playlist list underneath) invalidates the anchor position.
    const onScroll = (e: Event) => {
      if (el && e.composedPath().includes(el)) return;
      onclose();
    };
    window.addEventListener("pointerdown", onDown, true);
    window.addEventListener("resize", dismiss);
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("blur", dismiss);
    return () => {
      unregisterEscape();
      window.removeEventListener("pointerdown", onDown, true);
      window.removeEventListener("resize", dismiss);
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("blur", dismiss);
    };
  });
</script>

<div
  class="menu"
  bind:this={el}
  use:portal
  style="left:{left}px; top:{top}px; max-width:calc(100vw - {8 * scale}px); max-height:calc(100vh - {8 * scale}px); --s:{scale};"
  role="menu">
  {#if info.length > 0}
    <div class="info" role="group" aria-label="track details">
      {#each info as row, i (i)}
        <div class="irow">
          <span class="ilabel">{row.label}</span>
          <span class="ivalue" title={row.value}>{row.value}</span>
        </div>
      {/each}
    </div>
    <div class="sep" role="separator"></div>
  {/if}
  {#each items as item, i (i)}
    {#if item.separator}
      <div class="sep" role="separator"></div>
    {:else if item.rating !== undefined}
      <div class="rating" role="group" aria-label="rating">
        <span class="rlabel">rating</span>
        <span class="stars">
          {#each [1, 2, 3, 4, 5] as n (n)}
            <button
              class="star"
              class:on={item.rating >= n}
              title={item.rating === n ? `${n} stars (click to clear)` : `${n} star${n === 1 ? "" : "s"}`}
              onclick={() => onselect(item.rating === n ? "rate:0" : `rate:${n}`)}>★</button>
          {/each}
        </span>
      </div>
    {:else}
      <button
        class="item"
        class:danger={item.danger}
        role="menuitem"
        disabled={item.disabled}
        onclick={() => onselect(item.id)}>
        {item.label}
      </button>
    {/if}
  {/each}
</div>

<style>
  .menu {
    position: fixed;
    z-index: 1000;
    /* The playlist action menu sits inside a pointer-events:none overlay. */
    pointer-events: auto;
    min-width: calc(170px * var(--s, 1));
    /* The playlist window is only 275px wide; keep the fixed menu inside it
       so long values wrap against the visible width instead of being clipped
       by the window edge. */
    max-width: calc(100vw - 8px);
    max-height: calc(100vh - 8px);
    box-sizing: border-box;
    padding: calc(3px * var(--s, 1)) 0;
    background: #161616;
    border: calc(1px * var(--s, 1)) solid #3a3a3a;
    box-shadow: calc(2px * var(--s, 1)) calc(2px * var(--s, 1)) 0 #000;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: calc(11px * var(--s, 1));
    color: #c8c8c8;
    user-select: none;
    /* The playlist window is only ~230px tall; a details block + actions can
       exceed it, so the menu scrolls instead of clipping. */
    overflow-y: auto;
    overscroll-behavior: contain;
    scrollbar-width: thin;
    scrollbar-color: #2f4f33 #161616;
  }

  /* Match the playlist's thin scrollbar on WebKit too. */
  .menu::-webkit-scrollbar {
    width: calc(6px * var(--s, 1));
  }

  .menu::-webkit-scrollbar-thumb {
    background: #2f4f33;
  }

  .info {
    max-width: calc(360px * var(--s, 1));
    box-sizing: border-box;
    padding: calc(3px * var(--s, 1)) calc(10px * var(--s, 1)) calc(5px * var(--s, 1));
  }

  .irow {
    display: flex;
    gap: calc(8px * var(--s, 1));
    line-height: 1.5;
  }

  .ilabel {
    flex: none;
    width: calc(52px * var(--s, 1));
    color: #707070;
  }

  .ivalue {
    min-width: 0;
    /* Wrap inside the menu's viewport-capped width: album titles and quality
       strings can be much longer than the playlist window. */
    overflow-wrap: anywhere;
    color: #a8a8b0;
  }

  .item {
    display: block;
    width: 100%;
    box-sizing: border-box;
    text-align: left;
    background: transparent;
    color: inherit;
    border: none;
    padding: calc(5px * var(--s, 1)) calc(10px * var(--s, 1));
    font: inherit;
    cursor: pointer;
  }

  .item:hover:not(:disabled) {
    background: #12301b;
    color: #8dff9e;
  }

  .item.danger {
    color: #d8604c;
  }

  .item.danger:hover:not(:disabled) {
    background: #2a120e;
    color: #ff7a64;
  }

  .item:disabled {
    color: #555;
    cursor: default;
  }

  .sep {
    height: calc(1px * var(--s, 1));
    margin: calc(3px * var(--s, 1)) 0;
    background: #2a2a2a;
  }

  .rating {
    display: flex;
    align-items: center;
    gap: calc(8px * var(--s, 1));
    padding: calc(4px * var(--s, 1)) calc(10px * var(--s, 1));
  }

  .rlabel {
    color: #808080;
  }

  .stars {
    display: flex;
    gap: calc(1px * var(--s, 1));
  }

  .star {
    background: none;
    border: none;
    padding: 0 calc(1px * var(--s, 1));
    font-size: calc(13px * var(--s, 1));
    line-height: 1;
    color: #3a5a3f;
    cursor: pointer;
  }

  .star:hover,
  .star.on {
    color: #d8a63c;
  }
</style>
