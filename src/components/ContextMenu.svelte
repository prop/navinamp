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
  interface Props {
    x: number;
    y: number;
    items: ContextMenuItem[];
    onselect: (id: string) => void;
    onclose: () => void;
  }

  let { x, y, items, onselect, onclose }: Props = $props();

  let el = $state<HTMLDivElement | null>(null);
  // Initial placement; the effect below clamps it after measuring. The
  // component remounts per open, so capturing x/y once is intended.
  // svelte-ignore state_referenced_locally
  let left = $state(x);
  // svelte-ignore state_referenced_locally
  let top = $state(y);

  // Clamp after measuring (the menu width depends on its longest label).
  $effect(() => {
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
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onclose();
      }
    };
    const dismiss = () => onclose();
    window.addEventListener("pointerdown", onDown, true);
    window.addEventListener("keydown", onKey);
    window.addEventListener("resize", dismiss);
    window.addEventListener("scroll", dismiss, true);
    window.addEventListener("blur", dismiss);
    return () => {
      window.removeEventListener("pointerdown", onDown, true);
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("resize", dismiss);
      window.removeEventListener("scroll", dismiss, true);
      window.removeEventListener("blur", dismiss);
    };
  });
</script>

<div class="menu" bind:this={el} style="left:{left}px; top:{top}px;" role="menu">
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
    padding: 3px 0;
    background: #161616;
    border: 1px solid #3a3a3a;
    box-shadow: 2px 2px 0 #000;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 11px;
    color: #c8c8c8;
    user-select: none;
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
