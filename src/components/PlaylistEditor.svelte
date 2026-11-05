<script lang="ts">
  /**
   * Winamp3 Playlist Editor *flavor* (the M3 interaction panel). Fixed-width
   * panel whose LIST AREA is themed from the loaded skin's pledit.txt colors
   * and font (the `theme` prop: normal/current text, list bg, selected bg,
   * font — classic green-on-black fallback). When a skin has a playlist
   * container the window embeds this component (`embedded`) inside the skin
   * frame and hides the HTML footer/border. Pure props/callbacks — the
   * component knows nothing about the queue module's state or IPC.
   *
   * Interactions: click selects, double-click plays, pointer-drag reorders
   * (drop indicator, no live reshuffle — simple and robust), Delete removes
   * the selection, ↑/↓ move the selection. ADD / ADD DIR open the caller's
   * dialogs; ADD URL adds an http(s) stream; REM removes the selection.
   */
  import { isStreamUrl, type QueueItem } from "../lib/player/queue.svelte";
  import { mixHex, type SkinTheme } from "../lib/skin/theme";
  import ContextMenu, { type ContextMenuItem } from "./ContextMenu.svelte";
  import { clearEqFor, getArtistEq, getTrackEq, rememberEq } from "../lib/audio/auto";
  import { librarySetRating, libraryTrack, type Track } from "../lib/library/ipc";
  import { navidromeRate, navidromeStar } from "../lib/navidrome/ipc";

  interface Props {
    items: QueueItem[];
    currentIndex: number | null;
    /** pledit.txt colors from the loaded skin (defaults handled by caller). */
    theme?: SkinTheme;
    /**
     * Selected row index (bindable): the skinned playlist window drives the
     * skin's REM/SEL action buttons from it.
     */
    selected?: number | null;
    /**
     * Embedded mode: fill the caller's box and hide the HTML footer/border
     * (the skin frame owns the chrome and action buttons).
     */
    embedded?: boolean;
    /** Row height in CSS pixels (classic PE rows are tighter than the panel). */
    rowHeight?: number;
    /** Visual scale of the surrounding skin frame (pointer math only). */
    scale?: number;
    onplay: (index: number) => void;
    onremove: (index: number) => void;
    onmove: (from: number, to: number) => void;
    onadd: () => void;
    onadddir: () => void;
    onaddurl: () => void;
  }

  let {
    items,
    currentIndex,
    theme,
    selected = $bindable(null),
    embedded = false,
    rowHeight = 20,
    scale = 1,
    onplay,
    onremove,
    onmove,
    onadd,
    onadddir,
    onaddurl,
  }: Props = $props();

  /** Must match the rendered .row height so drag math stays trivial. */
  const ROW_H = $derived(rowHeight);
  /** Extra rows above/below the viewport (smooth fast scrolling). */
  const OVERSCAN = 8;
  let listEl: HTMLElement | null = null;

  // Windowed rendering: PLAY ALL on a 50k-track library must not create 50k
  // DOM rows. Only the visible slice (+overscan) is mounted; a full-height
  // spacer keeps the scrollbar and drag geometry in whole-list coordinates.
  let scrollTop = $state(0);
  let viewportH = $state(160);
  const totalH = $derived(items.length * ROW_H);
  const start = $derived(
    Math.max(0, Math.min(items.length, Math.floor(scrollTop / ROW_H) - OVERSCAN)),
  );
  const end = $derived(
    Math.min(
      items.length,
      Math.max(start, Math.ceil((scrollTop + viewportH) / ROW_H) + OVERSCAN),
    ),
  );
  const visible = $derived(items.slice(start, end));

  $effect(() => {
    const el = listEl;
    if (!el) return;
    viewportH = el.clientHeight;
    const ro = new ResizeObserver(() => {
      viewportH = el.clientHeight;
    });
    ro.observe(el);
    return () => ro.disconnect();
  });

  // The queue can shrink under the current scroll position (clear, removals
  // from elsewhere); keep the virtual window inside the new bounds.
  $effect(() => {
    const el = listEl;
    if (!el) return;
    const max = Math.max(0, items.length * ROW_H - el.clientHeight);
    if (el.scrollTop > max) {
      el.scrollTop = max;
      scrollTop = max;
    }
  });

  function onScroll(e: Event & { currentTarget: HTMLElement }): void {
    scrollTop = e.currentTarget.scrollTop;
  }

  /** Keep the selected row mounted/visible after keyboard navigation. */
  function ensureVisible(index: number): void {
    scrollToIndex(index);
  }

  /** Scroll one row into view; `center` is used by the NOW button. */
  function scrollToIndex(index: number, center = false): void {
    const el = listEl;
    if (!el) return;
    const top = index * ROW_H;
    if (center) {
      el.scrollTop = Math.max(0, top - (el.clientHeight - ROW_H) / 2);
    } else if (top < el.scrollTop) {
      el.scrollTop = top;
    } else if (top + ROW_H > el.scrollTop + el.clientHeight) {
      el.scrollTop = top + ROW_H - el.clientHeight;
    }
  }

  // A selection set from OUTSIDE the list (the skinned SEL action selects
  // the playing row) must also scroll into view; clicks are already visible.
  $effect(() => {
    if (selected != null) ensureVisible(selected);
  });

  /** Jump to the playing row (the queue can be tens of thousands of rows). */
  function goToCurrent(): void {
    if (currentIndex == null) return;
    selected = currentIndex;
    scrollToIndex(currentIndex, true);
    listEl?.focus();
  }

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
    // clientY is screen-space; the list may be CSS-scaled by the skin frame.
    const y = (e.clientY - r.top) / scale + el.scrollTop;
    return Math.max(0, Math.min(items.length, Math.round(y / ROW_H)));
  }

  // Insertion gap → final index of the dragged item.
  function dropIndex(from: number, gap: number): number {
    return gap > from ? gap - 1 : gap;
  }

  function detachDrag(): void {
    window.removeEventListener("pointermove", rowMove);
    window.removeEventListener("pointerup", rowUp);
    window.removeEventListener("pointercancel", cancelDrag);
  }

  function cancelDrag(): void {
    detachDrag();
    drag = null;
  }

  function rowDown(e: PointerEvent, i: number): void {
    if (e.button !== 0) return;
    selected = i;
    drag = { from: i, gap: i, moved: false };
    dragStartY = e.clientY;
    // Window listeners (not pointer capture) keep the drag alive when the
    // source row scrolls out of the virtual window and unmounts.
    window.addEventListener("pointermove", rowMove);
    window.addEventListener("pointerup", rowUp);
    window.addEventListener("pointercancel", cancelDrag);
  }

  function rowMove(e: PointerEvent): void {
    if (!drag) return;
    if (!drag.moved && Math.abs(e.clientY - dragStartY) < 4 * scale) return;
    drag.moved = true;
    drag.gap = gapFrom(e);
  }

  function rowUp(): void {
    detachDrag();
    const current = drag;
    drag = null;
    if (!current || !current.moved) return; // plain click selected on down
    const to = dropIndex(current.from, current.gap);
    if (to !== current.from) onmove(current.from, to);
  }

  // A window close/tab teardown mid-drag must not leave global listeners.
  $effect(() => {
    return () => detachDrag();
  });

  // ── right-click menu (play / remove / star / rating) ─────────────────────
  // Playlist items carry no star/rating state, so the open menu looks the row
  // up in the library (fetch once per path, then cached in this window).

  interface StarInfo {
    starred: boolean;
    rating: number;
  }
  interface MenuState {
    x: number;
    y: number;
    index: number;
    /** Queue identity guard for async detail lookups. */
    path: string;
  }

  let menu = $state<MenuState | null>(null);
  let menuInfo = $state<StarInfo | null>(null);
  let menuTrack = $state<Track | null>(null);
  const starCache = new Map<string, StarInfo>();
  /** Full library row per path; `null` caches "not indexed". */
  const trackCache = new Map<string, Track | null>();

  /** Non-interactive details block for the open menu (queue fallbacks). */
  const menuDetails = $derived.by(() => {
    const m = menu;
    if (!m) return [];
    const item = items[m.index];
    if (!item) return [];
    const t = menuTrack;
    const rows: { label: string; value: string }[] = [];
    const add = (label: string, value: string | null | undefined): void => {
      if (value != null && value.trim() !== "") rows.push({ label, value });
    };

    add("title", t?.title || item.title);
    add("artist", t?.artist || item.artist);
    add(
      "album",
      t
        ? [t.album, t.year != null ? `(${t.year})` : ""].filter(Boolean).join(" ")
        : null,
    );
    add("genre", t?.genre);

    const secs = t?.durationSecs ?? item.durationSecs;
    const trackNo =
      t?.trackNo != null
        ? `track ${t.discNo != null ? `${t.discNo}.` : ""}${t.trackNo}`
        : "";
    add("length", [secs != null ? fmt(secs) : "", trackNo].filter(Boolean).join(" · "));

    add(
      "quality",
      [
        t?.codec ? t.codec.toUpperCase() : "",
        t?.bitrateKbps != null ? `${t.bitrateKbps} kbps` : "",
        t?.samplerate != null ? `${(t.samplerate / 1000).toFixed(1)} kHz` : "",
        t?.channels === 1 ? "mono" : t?.channels === 2 ? "stereo" : "",
      ]
        .filter(Boolean)
        .join(" · "),
    );

    if (t) {
      const added =
        t.dateAdded > 0 ? new Date(t.dateAdded * 1000).toLocaleDateString() : "";
      add(
        "plays",
        [`${t.playCount}×`, added ? `added ${added}` : ""].filter(Boolean).join(" · "),
      );
      add("source", t.source === "navidrome" ? "Navidrome" : "Local");
    } else if (isStreamUrl(item.path)) {
      add(
        "source",
        item.streamKind === "youtube" ? "YouTube · yt-dlp" : "stream URL",
      );
    } else {
      add("source", "not in library");
    }
    return rows;
  });

  const menuItems = $derived.by((): ContextMenuItem[] => {
    const m = menu;
    if (!m) return [];
    const item = items[m.index];
    if (!item) return [];
    const stream = isStreamUrl(item.path);
    const list: ContextMenuItem[] = [
      { id: "play", label: "Play" },
      { id: "remove", label: "Remove from queue", danger: true },
      { id: "copy", label: stream ? "Copy URL" : "Copy path" },
    ];
    if (item.remoteId != null) {
      list.push({ id: "sep", label: "", separator: true });
      if (menuInfo == null) {
        list.push({ id: "star-loading", label: "loading star state…", disabled: true });
      } else {
        list.push({
          id: "star",
          label: menuInfo.starred ? "Unstar on Navidrome" : "Star on Navidrome",
        });
        list.push({ id: "rate", label: "rating", rating: menuInfo.rating });
      }
    }
    // AUTO associations; the EQ window reapplies the saved curve on match.
    const trackEq = getTrackEq(item.path) != null;
    const artistEq = item.artist.trim() !== "" && getArtistEq(item.artist) != null;
    list.push({ id: "eqsep", label: "", separator: true });
    list.push({
      id: "eq-track",
      label: trackEq ? "EQ: update track curve" : "EQ: save current curve for track",
    });
    list.push({
      id: "eq-artist",
      label: artistEq ? "EQ: update artist curve" : "EQ: save current curve for artist",
    });
    if (trackEq || artistEq) {
      list.push({ id: "eq-clear", label: "EQ: clear track/artist curve" });
    }
    return list;
  });

  function openMenu(e: MouseEvent, index: number): void {
    e.preventDefault();
    const item = items[index];
    if (!item) return;
    selected = index;
    const stream = isStreamUrl(item.path);
    const cachedTrack = stream ? null : (trackCache.get(item.path) ?? null);
    const cachedStar = starCache.get(item.path) ?? null;
    menuTrack = cachedTrack;
    menuInfo = item.remoteId != null ? cachedStar : null;
    menu = { x: e.clientX, y: e.clientY, index, path: item.path };

    // Stream rows are not library rows: skip the DB lookup entirely.
    const needsTrack = !stream && !trackCache.has(item.path);
    const needsStar = item.remoteId != null && cachedStar == null;
    if (!needsTrack && !needsStar) return;
    void libraryTrack(item.path)
      .then((track) => {
        trackCache.set(item.path, track);
        const info = {
          starred: track?.starred ?? false,
          rating: track?.rating ?? 0,
        };
        if (item.remoteId != null) starCache.set(item.path, info);
        if (menu?.path === item.path) {
          menuTrack = track;
          if (item.remoteId != null) menuInfo = info;
        }
      })
      .catch(() => {
        trackCache.set(item.path, null);
        if (menu?.path === item.path) {
          menuTrack = null;
          if (item.remoteId != null) menuInfo = { starred: false, rating: 0 };
        }
      });
  }

  async function mirrorStar(item: QueueItem, starred: boolean): Promise<void> {
    if (item.remoteId == null) return;
    const previous = starCache.get(item.path) ?? { starred: !starred, rating: 0 };
    const next = { ...previous, starred };
    starCache.set(item.path, next);
    menuInfo = next;
    try {
      await navidromeStar(item.serverId, item.remoteId, starred);
    } catch (e) {
      starCache.set(item.path, previous);
      menuInfo = previous;
      console.warn(`[navinamp] star failed: ${e}`);
    }
  }

  async function setItemRating(item: QueueItem, rating: number): Promise<void> {
    const previous = starCache.get(item.path) ?? { starred: false, rating: 0 };
    const next = { ...previous, rating };
    starCache.set(item.path, next);
    menuInfo = next;
    try {
      await librarySetRating(item.path, rating);
    } catch (e) {
      starCache.set(item.path, previous);
      menuInfo = previous;
      console.warn(`[navinamp] rating failed: ${e}`);
      return;
    }
    if (item.remoteId != null) {
      try {
        await navidromeRate(item.serverId, item.remoteId, rating);
      } catch (e) {
        console.warn(`[navinamp] rating push failed (kept locally): ${e}`);
      }
    }
  }

  function menuSelect(id: string): void {
    const m = menu;
    menu = null;
    if (!m) return;
    const item = items[m.index];
    if (!item) return;
    if (id === "play") {
      onplay(m.index);
    } else if (id === "remove") {
      onremove(m.index);
    } else if (id === "copy") {
      // Remote rows carry the server's real file path from enqueue time
      // (toMeta); the navidrome:// identity is the fallback for items
      // enqueued before the row had a path. Synchronous on purpose — a
      // cache lookup here would race the menu's async library fetch.
      const copyPath = item.remotePath ?? item.path;
      void navigator.clipboard.writeText(copyPath).catch((e) => {
        console.warn(`[navinamp] copy path failed: ${e}`);
      });
    } else if (id === "star") {
      void mirrorStar(item, !(menuInfo?.starred ?? false));
    } else if (id.startsWith("rate:")) {
      const rating = Number(id.slice("rate:".length));
      if (Number.isFinite(rating)) void setItemRating(item, rating);
    } else if (id === "eq-track" || id === "eq-artist") {
      void rememberEq(
        item.path,
        item.artist,
        id === "eq-track" ? "track" : "artist",
        item.title,
      ).catch((e) => console.warn(`[navinamp] eq: save failed: ${e}`));
    } else if (id === "eq-clear") {
      void clearEqFor(item.path, item.artist)
        .catch((e) => console.warn(`[navinamp] eq: clear failed: ${e}`));
    }
  }

  function listKey(e: KeyboardEvent & { currentTarget: HTMLElement }): void {
    if (items.length === 0) return;
    switch (e.key) {
      case "ArrowUp":
      case "ArrowDown": {
        e.preventDefault();
        const d = e.key === "ArrowUp" ? -1 : 1;
        const next =
          selected == null
            ? d > 0
              ? 0
              : items.length - 1
            : Math.max(0, Math.min(items.length - 1, selected + d));
        selected = next;
        ensureVisible(next);
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

  // pledit theme as CSS custom properties (classic-green defaults cover a
  // theme-less caller; every rule below reads only these variables).
  const themeVars = $derived.by(() => {
    const t = theme;
    const normal = t?.normal ?? "#00ff00";
    const current = t?.current ?? "#ffffff";
    const bg = t?.normalbg ?? "#000000";
    return [
      `--pl-normal:${normal}`,
      `--pl-current:${current}`,
      `--pl-bg:${bg}`,
      `--pl-selected-bg:${t?.selectedbg ?? "#0000c6"}`,
      `--pl-marker:${t?.marker ?? current}`,
      `--pl-border:${mixHex(normal, bg, 0.35)}`,
      `--pl-thumb:${mixHex(normal, bg, 0.55)}`,
      `--pl-hover:${mixHex(bg, "#ffffff", 0.08)}`,
      `--pl-font:${
        t?.font
          ? `${t.font}, Tahoma, sans-serif`
          : 'Tahoma, "Segoe UI", Geneva, Verdana, sans-serif'
      }`,
      `--row-h:${rowHeight}px`,
    ].join(";");
  });
</script>

<div class="pl" class:embedded style={themeVars}>
  <div
    class="list"
    bind:this={listEl}
    tabindex="0"
    role="listbox"
    aria-label="playlist"
    onkeydown={listKey}
    onscroll={onScroll}>
    <div class="rows" style="height:{totalH}px">
      {#if items.length === 0}
        <div class="empty">playlist empty — ADD or eject files</div>
      {:else}
        <div class="vspacer" style="height:{start * ROW_H}px" aria-hidden="true" role="presentation"></div>
        {#each visible as item, j (item.id)}
          {@const i = start + j}
          <div
            class="row"
            class:current={i === currentIndex}
            class:selected={i === selected}
            role="option"
            aria-selected={i === selected}
            tabindex="-1"
            onpointerdown={(e) => rowDown(e, i)}
            ondblclick={() => onplay(i)}
            oncontextmenu={(e) => openMenu(e, i)}
          >
            <span class="marker">{i === currentIndex ? "▶" : ""}</span>
            <span class="text">{i + 1}. {label(item)}</span>
            <span class="time">{item.durationSecs != null ? fmt(item.durationSecs) : ""}</span>
          </div>
        {/each}
        <div
          class="vspacer"
          style="height:{(items.length - end) * ROW_H}px"
          aria-hidden="true"
          role="presentation"></div>
      {/if}
      {#if drag && drag.moved && dropIndex(drag.from, drag.gap) !== drag.from}
        <div class="dropline" style="top:{drag.gap * ROW_H - 1}px"></div>
      {/if}
    </div>
  </div>

  {#if !embedded}
    <footer class="bar">
      <button onclick={() => onadd()}>ADD</button>
      <button onclick={() => onadddir()} title="add a folder recursively">ADD DIR</button>
      <button onclick={() => onaddurl()} title="add an http(s) audio stream or YouTube link">
        ADD URL
      </button>
      <button
        onclick={() => selected != null && onremove(selected)}
        disabled={selected == null}>REM</button
      >
      <button
        onclick={goToCurrent}
        disabled={currentIndex == null}
        title="scroll to the current queue item">NOW</button>
    </footer>
  {/if}
</div>

{#if menu}
  <ContextMenu
    x={menu.x}
    y={menu.y}
    items={menuItems}
    info={menuDetails}
    {scale}
    onselect={menuSelect}
    onclose={() => (menu = null)} />
{/if}

<style>
  .pl {
    width: 275px; /* matches the player window width */
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    border: 1px solid var(--pl-border, #3a3a3a);
    background: #161616;
    user-select: none;
  }

  /* Embedded in a skin frame: the caller owns the box, border and actions. */
  .pl.embedded {
    width: 100%;
    height: 100%;
    min-height: 0;
    border: none;
    background: transparent;
  }

  .list {
    position: relative;
    height: 160px; /* 8 rows */
    overflow-y: auto;
    background: var(--pl-bg);
    scrollbar-width: thin;
    scrollbar-color: var(--pl-thumb) var(--pl-bg);
  }

  .pl.embedded .list {
    height: 100%;
    flex: 1;
    min-height: 0;
  }

  .list:focus-visible {
    outline: 1px solid var(--pl-normal);
  }

  .list::-webkit-scrollbar {
    width: 6px;
  }

  .list::-webkit-scrollbar-thumb {
    background: var(--pl-thumb);
  }

  .rows {
    position: relative;
  }

  .vspacer {
    pointer-events: none;
  }

  /* Rows use the classic-editor treatment: small smooth UI text (real
     Winamp used Tahoma 8pt here), NOT a pixel font — pixel glyphs are for
     the LED displays only. Colors stay the skin's pledit palette. */
  .row {
    display: flex;
    align-items: center;
    height: var(--row-h, 20px);
    line-height: var(--row-h, 20px);
    padding: 0 6px;
    font-family: var(--pl-font);
    font-size: 11px;
    color: var(--pl-normal); /* classic Winamp playlist green by default */
    cursor: default;
    touch-action: none;
  }

  .row:hover {
    background: var(--pl-hover); /* subtle on any pledit bg */
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
    font-size: 10px;
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
    font-family: var(--pl-font);
    font-size: 11px;
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
    gap: 3px;
    padding: 3px 5px;
    background: var(--pl-bg);
    border-top: 1px solid var(--pl-border);
  }

  .bar button {
    background: transparent;
    color: var(--pl-normal);
    border: 1px solid var(--pl-border);
    border-radius: 2px;
    /* Five compact footer actions must fit the 275px playlist. */
    padding: 1px 4px;
    font-family: var(--pl-font);
    font-size: 10px;
    white-space: nowrap;
    cursor: pointer;
  }

  .bar button:hover:not(:disabled) {
    border-color: var(--pl-current);
    color: var(--pl-current);
  }

  .bar button:disabled {
    opacity: 0.35;
    cursor: default;
  }
</style>
