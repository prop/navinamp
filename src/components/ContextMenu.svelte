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
   * Rendered at the pointer's client coordinates and clamped to the
   * viewport. Closes on outside click, Escape, scroll, resize or focus loss.
   * Items are data-driven; a `rating` item renders a 5-star picker and emits
   * `rate:<n>` (or `rate:0` when the current star is clicked again).
   */
  import { registerEscape } from "../lib/window/escape";

  interface Props {
    x: number;
    y: number;
    items: ContextMenuItem[];
    /** Optional track details (artist, album, quality, …) above the actions. */
    info?: ContextMenuInfoRow[];
    onselect: (id: string) => void;
    onclose: () => void;
  }

  let { x, y, items, info = [], onselect, onclose }: Props = $props();

  let el = $state<HTMLDivElement | null>(null);
  // Initial placement; the effect below clamps it after measuring. The
  // component remounts per open, so capturing x/y once is intended.
  // svelte-ignore state_referenced_locally
  let left = $state(x);
  // svelte-ignore state_referenced_locally
  let top = $state(y);

  // Clamp after measuring (the menu width/height depend on content; the
  // details block can arrive asynchronously after the metadata lookup).
  $effect(() => {
    void info;
    const node = el;
    if (!node) return;
    const w = node.offsetWidth;
    const h = node.offsetHeight;
    left = Math.max(4, Math.min(x, window.innerWidth - w - 4));
    top = Math.max(4, Math.min(y, window.innerHeight - h - 4));
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

<div class="menu" bind:this={el} style="left:{left}px; top:{top}px;" role="menu">
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
    min-width: 170px;
    /* The playlist window is only 275px wide; keep the fixed menu inside it
       so long values wrap against the visible width instead of being clipped
       by the window edge. */
    max-width: calc(100vw - 8px);
    max-height: calc(100vh - 8px);
    box-sizing: border-box;
    padding: 3px 0;
    background: #161616;
    border: 1px solid #3a3a3a;
    box-shadow: 2px 2px 0 #000;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 11px;
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
    width: 6px;
  }

  .menu::-webkit-scrollbar-thumb {
    background: #2f4f33;
  }

  .info {
    max-width: 360px;
    box-sizing: border-box;
    padding: 3px 10px 5px;
  }

  .irow {
    display: flex;
    gap: 8px;
    line-height: 1.5;
  }

  .ilabel {
    flex: none;
    width: 52px;
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
    padding: 5px 10px;
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
    height: 1px;
    margin: 3px 0;
    background: #2a2a2a;
  }

  .rating {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 10px;
  }

  .rlabel {
    color: #808080;
  }

  .stars {
    display: flex;
    gap: 1px;
  }

  .star {
    background: none;
    border: none;
    padding: 0 1px;
    font-size: 13px;
    line-height: 1;
    color: #3a5a3f;
    cursor: pointer;
  }

  .star:hover,
  .star.on {
    color: #d8a63c;
  }
</style>
