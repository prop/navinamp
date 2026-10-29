<script lang="ts">
  /**
   * Winamp3 Playlist Editor *flavor* (the M3 interaction panel). Fixed-width
   * dark panel whose LIST AREA is themed from the loaded skin's pledit.txt
   * colors (the `theme` prop: normal/current text, list bg, selected bg —
   * classic green-on-black fallback) and set in a bundled OFL pixel font
   * (Silkscreen, src/assets/fonts). Pure props/callbacks — the component
   * knows nothing about the queue module's state or IPC.
   *
   * Interactions: click selects, double-click plays, pointer-drag reorders
   * (drop indicator, no live reshuffle — simple and robust), Delete removes
   * the selection, ↑/↓ move the selection. ADD opens the caller's dialog;
   * REM removes the selection; SEL/MISC are placeholders until M4.
   */
  import type { QueueItem } from "../lib/player/queue.svelte";
  import type { SkinTheme } from "../lib/skin/theme";

  interface Props {
    items: QueueItem[];
    currentIndex: number | null;
    /** pledit.txt colors from the loaded skin (defaults handled by caller). */
    theme?: SkinTheme;
    onplay: (index: number) => void;
    onremove: (index: number) => void;
    onmove: (from: number, to: number) => void;
    onadd: () => void;
    onadddir: () => void;
  }

  let {
    items,
    currentIndex,
    theme,
    onplay,
    onremove,
    onmove,
    onadd,
    onadddir,
  }: Props = $props();

  // Must match the .row height so drag math stays trivial.
  const ROW_H = 20;

  let selected = $state<number | null>(null);
  let listEl: HTMLElement | null = null;

  // `gap` is the insertion boundary (0..items.length) in pre-removal
  // coordinates; `moved` gates click-vs-drag on a small threshold.
  let drag = $state<{ from: number; gap: number; moved: boolean } | null>(null);
  let dragStartY = 0;

  // External shrinkage (clear, removals from elsewhere) heals the selection.
  $effect(() => {
    if (selected != null && selected >= items.length) {
      selected = items.length > 0 ? items.length - 1 : null;
    }
  });

  function fmt(secs: number): string {
    const m = Math.floor(secs / 60);
    const s = Math.floor(secs % 60);
    return `${m}:${String(s).padStart(2, "0")}`;
  }

  function label(it: QueueItem): string {
    return it.artist ? `${it.artist} - ${it.title}` : it.title;
  }

  function gapFrom(e: PointerEvent): number {
    const el = listEl;
    if (!el) return 0;
    const r = el.getBoundingClientRect();
    const y = e.clientY - r.top + el.scrollTop;
    return Math.max(0, Math.min(items.length, Math.round(y / ROW_H)));
  }

  // Insertion gap → final index of the dragged item.
  function dropIndex(from: number, gap: number): number {
    return gap > from ? gap - 1 : gap;
  }

  function rowDown(e: PointerEvent & { currentTarget: HTMLElement }, i: number): void {
    if (e.button !== 0) return;
    selected = i;
    drag = { from: i, gap: i, moved: false };
    dragStartY = e.clientY;
    // Capture so moves keep tracking this row even outside the list.
    e.currentTarget.setPointerCapture(e.pointerId);
  }

  function rowMove(e: PointerEvent): void {
    if (!drag) return;
    if (!drag.moved && Math.abs(e.clientY - dragStartY) < 4) return;
    drag.moved = true;
    drag.gap = gapFrom(e);
  }

  function rowUp(): void {
    if (!drag) return;
    const { from, gap, moved } = drag;
    drag = null;
    if (!moved) return; // plain click: selection already happened on down
    const to = dropIndex(from, gap);
    if (to !== from) onmove(from, to);
  }

  function listKey(e: KeyboardEvent & { currentTarget: HTMLElement }): void {
    if (items.length === 0) return;
    switch (e.key) {
      case "ArrowUp":
      case "ArrowDown": {
        e.preventDefault();
        const d = e.key === "ArrowUp" ? -1 : 1;
        selected =
          selected == null
            ? d > 0
              ? 0
              : items.length - 1
            : Math.max(0, Math.min(items.length - 1, selected + d));
        break;
      }
      case "Delete":
      case "Backspace":
        if (selected != null) {
          e.preventDefault();
          onremove(selected);
        }
        break;
    }
  }

  const totalText = $derived.by(() => {
    const secs = items.reduce((sum, it) => sum + (it.durationSecs ?? 0), 0);
    return `${items.length} item${items.length === 1 ? "" : "s"}, ${fmt(secs)}`;
  });

  // pledit theme as CSS custom properties (classic-green defaults cover a
  // theme-less caller; every rule below reads only these variables).
  const themeVars = $derived.by(() => {
    const t = theme;
    return [
      `--pl-normal:${t?.normal ?? "#00ff00"}`,
      `--pl-current:${t?.current ?? "#ffffff"}`,
      `--pl-bg:${t?.normalbg ?? "#000000"}`,
      `--pl-selected-bg:${t?.selectedbg ?? "#0000c6"}`,
      `--pl-marker:${t?.marker ?? t?.current ?? "#ffffff"}`,
    ].join(";");
  });
</script>

<div class="pl" style={themeVars}>
  <div class="list" bind:this={listEl} tabindex="0" role="listbox" aria-label="playlist" onkeydown={listKey}>
    <div class="rows">
      {#each items as item, i (item.id)}
        <div
          class="row"
          class:current={i === currentIndex}
          class:selected={i === selected}
          role="option"
          aria-selected={i === selected}
          tabindex="-1"
          onpointerdown={(e) => rowDown(e, i)}
          onpointermove={rowMove}
          onpointerup={rowUp}
          onpointercancel={() => (drag = null)}
          ondblclick={() => onplay(i)}
        >
          <span class="marker">{i === currentIndex ? "▶" : ""}</span>
          <span class="text">{i + 1}. {label(item)}</span>
          <span class="time">{item.durationSecs != null ? fmt(item.durationSecs) : ""}</span>
        </div>
      {/each}
      {#if items.length === 0}
        <div class="empty">playlist empty — ADD or eject files</div>
      {/if}
      {#if drag && drag.moved && dropIndex(drag.from, drag.gap) !== drag.from}
        <div class="dropline" style="top:{drag.gap * ROW_H - 1}px"></div>
      {/if}
    </div>
  </div>

  <footer class="bar">
    <button onclick={() => onadd()}>ADD</button>
    <button onclick={() => onadddir()} title="add a folder recursively">ADD DIR</button>
    <button
      onclick={() => selected != null && onremove(selected)}
      disabled={selected == null}>REM</button
    >
    <!-- TODO(M4): select helpers / sort & misc ops. -->
    <button disabled title="TODO(M4): selection helpers">SEL</button>
    <button disabled title="TODO(M4): sort and misc ops">MISC</button>
    <span class="totals">{totalText}</span>
  </footer>
</div>

<style>
  /* Bundled OFL pixel font (Jason Kottke's Silkscreen, designed for tiny UI
     text — the closest readable thing to the classic 8px playlist look;
     license: src/assets/fonts/silkscreen-OFL.txt). */
  @font-face {
    font-family: "Silkscreen";
    font-style: normal;
    font-weight: 400;
    font-display: swap;
    src: url("../assets/fonts/silkscreen-latin.woff2") format("woff2");
  }

  .pl {
    width: 275px; /* matches the player window width */
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    border: 1px solid #3a3a3a;
    background: #161616;
    user-select: none;
  }

  .list {
    position: relative;
    height: 160px; /* 8 rows */
    overflow-y: auto;
    background: var(--pl-bg);
    scrollbar-width: thin;
    scrollbar-color: #2f4f33 var(--pl-bg);
  }

  .list:focus-visible {
    outline: 1px solid var(--pl-normal);
  }

  .list::-webkit-scrollbar {
    width: 6px;
  }

  .list::-webkit-scrollbar-thumb {
    background: #2f4f33;
  }

  .rows {
    position: relative;
  }

  .row {
    display: flex;
    align-items: center;
    height: 20px;
    line-height: 20px;
    padding: 0 6px;
    font-family: "Silkscreen", ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 8px; /* Silkscreen's native pixel grid */
    letter-spacing: 0.02em;
    color: var(--pl-normal); /* classic Winamp playlist green by default */
    cursor: default;
    touch-action: none;
  }

  .row:hover {
    background: rgba(255, 255, 255, 0.06); /* subtle on any pledit bg */
  }

  .row.selected {
    background: var(--pl-selected-bg);
    color: var(--pl-current);
  }

  .row.current {
    color: var(--pl-current); /* pledit.txt Current — the playing row */
  }

  .marker {
    flex: none;
    width: 14px;
    color: var(--pl-marker);
    font-size: 8px;
  }

  .text {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .time {
    flex: none;
    width: 6ch;
    text-align: right;
    opacity: 0.85;
  }

  .empty {
    padding: 6px 8px;
    font-family: "Silkscreen", ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 8px;
    color: var(--pl-normal);
    opacity: 0.55;
  }

  .dropline {
    position: absolute;
    left: 0;
    right: 0;
    height: 2px;
    background: var(--pl-marker);
    pointer-events: none;
  }

  .bar {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 3px 6px;
    background: #101010;
    border-top: 1px solid #2a2a2a;
  }

  .bar button {
    background: #1c1c1c;
    color: #9a9a9a;
    border: 1px solid #3a3a3a;
    border-radius: 2px;
    padding: 1px 6px;
    font-size: 10px;
    letter-spacing: 0.08em;
    cursor: pointer;
  }

  .bar button:hover:not(:disabled) {
    border-color: #3ccb4e;
    color: #3ccb4e;
  }

  .bar button:disabled {
    opacity: 0.35;
    cursor: default;
  }

  .totals {
    margin-left: auto;
    font-family: "Silkscreen", ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 8px;
    color: #3ccb4e;
  }
</style>
