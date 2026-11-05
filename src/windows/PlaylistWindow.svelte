<script lang="ts">
  /**
   * The PLAYLIST EDITOR window (label "playlist") — a read-only view of the
   * queue snapshot with every mutation shipped to the player as a
   * queue-intent (M6b).
   *
   * Two chrome modes:
   *  - a skin with a `playlist` container (classic .wsz skins via the
   *    synthesizer, and the built-in fallback skin): the skin frame/titlebar
   *    is painted by SkinSurface and the HTML list is placed in the skin's
   *    list slot. The skin's own ADD/REM/SEL/MISC buttons drive the queue;
   *    ADD and MISC open small menus so the full action set stays available.
   *  - otherwise: the original Winamp3-flavored HTML panel, themed from
   *    pledit.txt, with its own footer.
   *
   * Local state stays local: selection / drag-reorder interactions are this
   * window's own; the current-item highlight comes from
   * snapshot.currentIndex, and durations render from the snapshot items
   * (the player patches them when TrackStarted reports the real length).
   */
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { open } from "@tauri-apps/plugin-dialog";

  import AddUrlDialog from "../components/AddUrlDialog.svelte";
  import ContextMenu, { type ContextMenuItem } from "../components/ContextMenu.svelte";
  import PlaylistEditor from "../components/PlaylistEditor.svelte";
  import SkinSurface from "../components/skin/SkinSurface.svelte";
  import { expandAudioPaths } from "../lib/library/ipc";
  import { formatTime, type QueueItemMeta } from "../lib/player/queue.svelte";
  import { loadDefaultSkin, onSkinChanged } from "../lib/skin/load";
  import { mockSkinPayload } from "../lib/skin/mock";
  import { resolveLayout, type ResolvedLayout, type ResolvedObject } from "../lib/skin/resolver";
  import { mixHex, type SkinTheme } from "../lib/skin/theme";
  import { dragRects } from "../lib/skin/titlebar";
  import type { SkinPayload } from "../lib/skin/types";
  import { dragWindow } from "../lib/window/drag";
  import {
    layoutMode,
    queueIntent,
    queueSnapshot,
    sizeWindowTo,
    skinTheme,
    uiScale,
    windowSetVisible,
    windowToggleLayout,
  } from "../lib/window/bus.svelte";
  import { LABEL } from "../lib/window/identity";
  import TitleStrip from "./TitleStrip.svelte";

  // Windowshade: the OS window manager owns the normal/shade flag; this
  // window hides its body and keeps the title strip when folded.
  const mode = $derived(layoutMode(LABEL));

  const snap = $derived(queueSnapshot.value);
  const items = $derived(snap?.items ?? []);
  const currentIndex = $derived(snap?.currentIndex ?? null);
  const shuffle = $derived(snap?.shuffle ?? false);
  const repeat = $derived(snap?.repeat ?? "off");

  let showAddUrl = $state(false);
  // Selection is owned by PlaylistEditor but bound here so the skin's own
  // REM/SEL action buttons can act on it.
  let selected = $state<number | null>(null);
  let menu = $state<{ kind: "add" | "misc"; x: number; y: number } | null>(null);

  function addUrl(meta: QueueItemMeta): void {
    queueIntent({ type: "addUrl", meta });
    showAddUrl = false;
  }

  // Folded strip: classic windowshade shows the playing track and its length.
  const stripTitle = $derived.by(() => {
    if (mode !== "shade" || currentIndex == null) return "PLAYLIST EDITOR";
    const item = items[currentIndex];
    if (!item) return "PLAYLIST EDITOR";
    const label = item.artist ? `${item.artist} - ${item.title}` : item.title;
    return `${currentIndex + 1}. ${label}`;
  });
  const stripNote = $derived.by(() => {
    if (mode !== "shade" || currentIndex == null) return undefined;
    const secs = items[currentIndex]?.durationSecs;
    return secs != null ? formatTime(secs) : undefined;
  });
  // pledit.txt colors of the loaded classic skin (classic-green default).
  const theme = $derived(skinTheme.value);

  // The footer total was removed to keep the 275px action row unclipped;
  // keep the same information on the position readout's tooltip.
  const queueTotal = $derived.by(() => {
    const secs = items.reduce((sum, item) => sum + (item.durationSecs ?? 0), 0);
    return `${items.length} item${items.length === 1 ? "" : "s"}, ${formatTime(secs)}`;
  });

  // ── skin loading (same persisted pick as the player/EQ) ─────────────────

  let skin = $state<SkinPayload | null>(null);
  // Global UI scale (DEV tab / Alt+S); all windows share it.
  const scale = $derived(uiScale.value);
  let skinSeq = 0;

  async function reloadSkin(): Promise<void> {
    const seq = ++skinSeq;
    let payload: SkinPayload;
    try {
      payload = await loadDefaultSkin();
    } catch (e) {
      console.warn(`[navinamp] skin load failed (${e}; using mock)`);
      payload = mockSkinPayload();
    }
    if (seq === skinSeq) skin = payload;
  }

  $effect(() => {
    void reloadSkin();
  });

  $effect(() => {
    const unlisten = onSkinChanged(() => void reloadSkin());
    return () => {
      void unlisten.then((f) => f());
    };
  });

  // The `playlist` container must exist explicitly: resolveLayout alone falls
  // back to the skin's FIRST container (the player window).
  const hasPlaylistContainer = $derived(
    skin?.skin.containers.some((c) => c.id === "playlist" && c.layouts.length > 0) ?? false,
  );

  /**
   * Static sample rows/ghost totals drawn by the built-in fallback skin.
   * Match exact ids (not a `playlist.` prefix) so third-party layers — and
   * `wsz.*` synthesizer content — are never swallowed.
   */
  const PLACEHOLDERS = new Set([
    "playlist.row.divider",
    "playlist.row.selected",
    "playlist.row.current",
    "playlist.totals.bg",
  ]);
  function isPlaceholder(o: ResolvedObject): boolean {
    return o.object.id != null && PLACEHOLDERS.has(o.object.id);
  }

  const skinLayout = $derived.by((): ResolvedLayout | null => {
    if (!skin || !hasPlaylistContainer) return null;
    const raw = resolveLayout(skin, "playlist", "normal");
    if (!raw) return null;
    return { ...raw, objects: raw.objects.filter((o) => !isPlaceholder(o)) };
  });

  interface Slot {
    x: number;
    y: number;
    w: number;
    h: number;
    /** Row height the skin wants for the HTML list (classic 16, fallback 20). */
    rowHeight: number;
  }

  function findSlot(layout: ResolvedLayout | null, ids: readonly string[]): Slot | null {
    if (!layout) return null;
    for (const o of layout.objects) {
      const id = o.object.id ?? "";
      if (!ids.includes(id) || o.w == null || o.h == null) continue;
      if (o.w <= 0 || o.h <= 0) continue;
      const requested = Number(o.object.params?.["rowheight"]);
      const rowHeight =
        Number.isFinite(requested) && requested >= 8 ? requested : 16;
      return { x: o.x, y: o.y, w: o.w, h: o.h, rowHeight };
    }
    return null;
  }

  const listSlot = $derived(
    findSlot(skinLayout, ["playlist.list", "playlist.list.bg", "wsz.pl.list"]),
  );
  const totalsSlot = $derived(findSlot(skinLayout, ["playlist.totals", "wsz.pl.totals"]));
  const useSkin = $derived(mode === "normal" && skinLayout != null && listSlot != null);

  // The action menus live in the skin overlay; fold/unskin drops them.
  $effect(() => {
    if (!useSkin) menu = null;
  });

  // Leave the skin's 2px recessed bevel visible around the HTML list.
  const listBox = $derived.by((): Slot | null => {
    const s = listSlot;
    if (!s) return null;
    return {
      x: s.x + 2,
      y: s.y + 2,
      w: Math.max(0, s.w - 4),
      h: Math.max(0, s.h - 4),
      rowHeight: s.rowHeight,
    };
  });

  const dragStrips = $derived.by(() =>
    skin && skinLayout ? dragRects(skin.skin, skinLayout) : [],
  );

  // Queue position in the skin's totals slot; shuffle/repeat/clear live in
  // the MISC menu (the skin's action bar has no room for a full qbar).
  const statusText = $derived(
    `${currentIndex == null ? "--" : currentIndex + 1}/${items.length}`,
  );

  // CSS vars shared by the skin status readout and the unskinned qbar.
  const chromeVars = $derived.by(() => {
    const t: SkinTheme = theme;
    return [
      `--pl-normal:${t.normal}`,
      `--pl-current:${t.current}`,
      `--pl-bg:${t.normalbg}`,
      `--pl-border:${mixHex(t.normal, t.normalbg, 0.35)}`,
      `--pl-font:${
        t.font
          ? `${t.font}, Tahoma, sans-serif`
          : 'Tahoma, "Segoe UI", Geneva, Verdana, sans-serif'
      }`,
    ].join(";");
  });

  function actionOf(o: ResolvedObject["object"]): string | null {
    if (o.kind !== "button" && o.kind !== "togglebutton" && o.kind !== "slider") {
      return null;
    }
    return (o.action ?? "").trim().toUpperCase() || null;
  }

  function actionAnchor(action: string): { x: number; y: number } {
    const target = skinLayout?.objects.find((o) => actionOf(o.object) === action);
    // Viewport pixels: the ContextMenu is inside the transformed overlay and
    // divides by `scale` before positioning (see openMenu).
    if (!target) return { x: 8 * scale, y: 8 * scale };
    return { x: target.x * scale, y: target.y * scale };
  }

  function openMenu(kind: "add" | "misc", action: string): void {
    const at = actionAnchor(action);
    menu = { kind, x: Math.round(at.x), y: Math.round(at.y) };
  }

  function removeSelected(): void {
    if (selected != null) queueIntent({ type: "remove", index: selected });
  }

  function skinAction(action: string): void {
    switch (action.trim().toUpperCase()) {
      case "PLAYLIST_ADD":
        openMenu("add", "PLAYLIST_ADD");
        break;
      case "PLAYLIST_REMOVE":
        removeSelected();
        break;
      case "PLAYLIST_SELECT":
        if (currentIndex != null) selected = currentIndex;
        break;
      case "PLAYLIST_MISC":
      case "PLAYLIST_LIST":
        openMenu("misc", action.trim().toUpperCase());
        break;
      case "TOGGLE_SHADE":
        windowToggleLayout(LABEL).catch(() => undefined);
        break;
      case "CLOSE":
        windowSetVisible(LABEL, false).catch(() => undefined);
        break;
      case "MINIMIZE":
        void getCurrentWindow().minimize().catch(() => undefined);
        break;
    }
  }

  const addMenuItems: ContextMenuItem[] = [
    { id: "add-files", label: "Add files…" },
    { id: "add-dir", label: "Add folder…" },
    { id: "add-url", label: "Add URL…" },
  ];

  const miscMenuItems = $derived.by((): ContextMenuItem[] => {
    return [
      { id: "now", label: "Jump to playing track", disabled: currentIndex == null },
      { id: "shuffle", label: shuffle ? "Shuffle: on" : "Shuffle: off" },
      { id: "repeat", label: `Repeat: ${repeat}` },
      { id: "sep", label: "", separator: true },
      { id: "add-dir", label: "Add folder…" },
      { id: "add-url", label: "Add URL…" },
      { id: "sep2", label: "", separator: true },
      { id: "clear", label: "Clear playlist", danger: true, disabled: items.length === 0 },
    ];
  });

  function menuSelect(id: string): void {
    menu = null;
    switch (id) {
      case "add-files":
        void addAfterCurrent();
        break;
      case "add-dir":
        void addDirAfterCurrent();
        break;
      case "add-url":
        showAddUrl = true;
        break;
      case "now":
        if (currentIndex != null) selected = currentIndex;
        break;
      case "shuffle":
        queueIntent({ type: "toggleShuffle" });
        break;
      case "repeat":
        queueIntent({ type: "cycleRepeat" });
        break;
      case "clear":
        queueIntent({ type: "clear" });
        break;
    }
  }

  // ADD: insert the picked files right after the current track. The indexes
  // are computed from the snapshot and sent explicitly; the player applies
  // the intents in emission order, so the block lands contiguously. (A
  // concurrent mutation elsewhere can still shift them — last writer wins.)
  async function addAfterCurrent(): Promise<void> {
    let res: string | string[] | null = null;
    try {
      res = await open({ multiple: true, directory: false });
    } catch {
      return;
    }
    const files = Array.isArray(res) ? res : res != null ? [res] : [];
    // Backend expands directories and filters to known audio extensions.
    const paths = await expandAudioPaths(files);
    if (paths.length === 0) return;
    let at = currentIndex == null ? items.length : currentIndex + 1;
    for (const path of paths) {
      queueIntent({ type: "addNext", path, index: at });
      at += 1;
    }
  }

  // ADD DIR: pick one folder; the backend walks it recursively (sorted).
  async function addDirAfterCurrent(): Promise<void> {
    let res: string | null = null;
    try {
      res = await open({ multiple: false, directory: true });
    } catch {
      return;
    }
    if (typeof res !== "string") return;
    const paths = await expandAudioPaths([res]);
    if (paths.length === 0) return;
    let at = currentIndex == null ? items.length : currentIndex + 1;
    for (const path of paths) {
      queueIntent({ type: "addNext", path, index: at });
      at += 1;
    }
  }

  // Ctrl/Cmd+U opens ADD URL (the playlist window owns the shortcut).
  $effect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (mode !== "normal") return;
      if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === "u") {
        e.preventDefault();
        showAddUrl = true;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  // Size the OS window directly from the skin layout (like the player
  // window) so the drag strips and the OS window can never drift. The
  // unskinned/shade panel instead observes its natural content size.
  let root: HTMLElement | null = $state(null);
  let lastW = 0;
  let lastH = 0;
  $effect(() => {
    if (useSkin && skinLayout) {
      const w = Math.ceil(skinLayout.w * scale);
      const h = Math.ceil(skinLayout.h * scale);
      if (w !== lastW || h !== lastH) {
        lastW = w;
        lastH = h;
        sizeWindowTo(LABEL, w, h);
      }
      return;
    }

    const el = root;
    if (!el) return;
    const apply = () => {
      const r = el.getBoundingClientRect();
      const w = Math.ceil(r.width);
      const h = Math.ceil(r.height);
      if (w === 0 || h === 0 || (w === lastW && h === lastH)) return;
      lastW = w;
      lastH = h;
      sizeWindowTo(LABEL, w, h);
    };
    apply();
    const ro = new ResizeObserver(apply);
    ro.observe(el);
    return () => ro.disconnect();
  });
</script>

<div class="win" class:shade={mode === "shade"} class:skinned={useSkin} bind:this={root}>
  {#if mode === "shade"}
    <!-- Classic windowshade: the strip is the playlist row itself. -->
    <TitleStrip
      title={stripTitle}
      note={stripNote}
      playlistTheme={theme}
      onfold={() => windowToggleLayout(LABEL).catch(() => undefined)}
    />
  {:else if useSkin && skin && skinLayout && listBox}
    <div
      class="skin-window"
      style="width:{skinLayout.w * scale}px; height:{skinLayout.h * scale}px;">
      <SkinSurface layout={skinLayout} {scale} onaction={skinAction} />

      <!-- The overlay lives in skin-pixel space; the CSS scale matches
           SkinSurface's bitmap scale. Only the list catches pointers. -->
      <div
        class="overlay"
        style="width:{skinLayout.w}px; height:{skinLayout.h}px; transform:scale({scale});{chromeVars}">
        <div
          class="list-slot"
          style="left:{listBox.x}px; top:{listBox.y}px; width:{listBox.w}px; height:{listBox.h}px;">
          <PlaylistEditor
            items={items}
            {currentIndex}
            {theme}
            embedded
            rowHeight={listBox.rowHeight}
            {scale}
            bind:selected
            onplay={(i) => queueIntent({ type: "play", index: i })}
            onremove={(i) => queueIntent({ type: "remove", index: i })}
            onmove={(from, to) => queueIntent({ type: "move", from, to })}
            onadd={() => void addAfterCurrent()}
            onadddir={() => void addDirAfterCurrent()}
            onaddurl={() => (showAddUrl = true)}
          />
        </div>

        {#if totalsSlot}
          <div
            class="skin-status"
            style="left:{totalsSlot.x}px; top:{totalsSlot.y}px; width:{totalsSlot.w}px; height:{totalsSlot.h}px; line-height:{totalsSlot.h}px; font-size:{totalsSlot.w < 90 ? 8 : 9}px;"
            title={queueTotal}>
            {statusText}
          </div>
        {/if}

        {#if menu}
          <ContextMenu
            x={menu.x}
            y={menu.y}
            items={menu.kind === "add" ? addMenuItems : miscMenuItems}
            {scale}
            onselect={menuSelect}
            onclose={() => (menu = null)} />
        {/if}
      </div>

      <div class="drag-layer" aria-hidden="true">
        {#each dragStrips as r (`${r.x}:${r.y}`)}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="drag"
            use:dragWindow
            style="left:{r.x * scale}px; top:{r.y * scale}px; width:{r.w * scale}px; height:{r.h * scale}px;"
            title="Drag to move — double-click toggles shade"
            ondblclick={() => windowToggleLayout(LABEL).catch(() => undefined)}
          ></div>
        {/each}
      </div>
    </div>
  {:else}
    <!-- No playlist container in the loaded skin: the palette-themed panel. -->
    <TitleStrip
      title="PLAYLIST EDITOR"
      playlistTheme={theme}
      onfold={() => windowToggleLayout(LABEL).catch(() => undefined)}
    />
    <div class="qbar mono" style={chromeVars}>
      <span
        class="pos"
        title={queueTotal}
        aria-label={currentIndex == null
          ? `queue empty; ${queueTotal}`
          : `track ${currentIndex + 1} of ${items.length}; ${queueTotal}`}
        >{currentIndex == null ? "--" : currentIndex + 1}/{items.length}</span
      >
      <button class:on={shuffle} onclick={() => queueIntent({ type: "toggleShuffle" })}>shuffle</button>
      <button class:on={repeat !== "off"} onclick={() => queueIntent({ type: "cycleRepeat" })}>
        repeat&nbsp;{repeat}
      </button>
      <button onclick={() => queueIntent({ type: "clear" })} disabled={items.length === 0}>clear</button>
    </div>

    <PlaylistEditor
      {items}
      {currentIndex}
      {theme}
      bind:selected
      onplay={(i) => queueIntent({ type: "play", index: i })}
      onremove={(i) => queueIntent({ type: "remove", index: i })}
      onmove={(from, to) => queueIntent({ type: "move", from, to })}
      onadd={() => void addAfterCurrent()}
      onadddir={() => void addDirAfterCurrent()}
      onaddurl={() => (showAddUrl = true)}
    />
  {/if}

  {#if showAddUrl}
    <AddUrlDialog onsubmit={addUrl} onclose={() => (showAddUrl = false)} />
  {/if}
</div>

<style>
  .win {
    /* Match the 275px player/editor width. A fixed width keeps the OS window
       from growing to fit the folded strip's (long) current-track title. */
    width: 275px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 4px 0;
    box-sizing: border-box;
    background: #000;
    user-select: none;
  }

  /* Skinned mode: the skin bitmap owns the exact window box. */
  .win.skinned {
    width: max-content;
    padding: 0;
    gap: 0;
  }

  /* Classic windowshade is exactly the 14px strip; the normal-mode frame
     padding would otherwise make it 22px tall. */
  .win.shade {
    padding: 0;
  }

  .skin-window {
    position: relative;
    overflow: hidden;
  }

  .overlay {
    position: absolute;
    inset: 0;
    transform-origin: top left;
    pointer-events: none;
    /* Above the drag layer so an open row/action menu can overlap the
       titlebar and still receive clicks (its children re-enable events). */
    z-index: 11;
  }

  .list-slot {
    position: absolute;
    pointer-events: auto;
  }

  .skin-status {
    position: absolute;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    padding-left: 2px;
    box-sizing: border-box;
    font-family: var(--pl-font);
    font-size: 9px;
    letter-spacing: 0.02em;
    color: var(--pl-normal);
    text-shadow: 0 1px 0 rgb(0 0 0 / 55%);
    pointer-events: auto;
    cursor: default;
  }

  /* Transparent layer above the skin; only the titlebar strips drag. */
  .drag-layer {
    position: absolute;
    inset: 0;
    pointer-events: none;
    z-index: 10;
  }

  .drag {
    position: absolute;
    pointer-events: auto;
    cursor: default;
  }

  .qbar {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 275px;
    box-sizing: border-box;
    font-size: 0.75rem;
    font-family: var(--pl-font);
    color: var(--pl-normal);
  }

  .pos {
    min-width: 7ch;
    color: var(--pl-current);
  }

  .qbar button {
    background: transparent;
    color: var(--pl-normal);
    border: 1px solid var(--pl-border);
    border-radius: 3px;
    padding: 0.15rem 0.5rem;
    font-size: 0.7rem;
    letter-spacing: 0.08em;
    cursor: pointer;
  }

  .qbar button:hover:not(:disabled) {
    border-color: var(--pl-current);
    color: var(--pl-current);
  }

  .qbar button:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .qbar button.on {
    border-color: var(--pl-current);
    color: var(--pl-current);
  }

  .mono {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
  }
</style>
