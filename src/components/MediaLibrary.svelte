<script lang="ts">
  /**
   * Winamp3 Media Library *flavor* (M4 panel; M6 swaps in a skinned window).
   *
   * Layout: scan-management strip → top bar (FTS search + enqueue actions +
   * stats) → body (left nav tree / right track table) → footer. Same visual
   * language as PlaylistEditor (dark panel, mono green rows, 10px caps
   * chrome). Pure props for queue mutations — the app wires them to
   * `queue.addFromLibrary`, keeping this component queue-agnostic.
   *
   * Data flow:
   *  · one `refreshAll()` loads stats/roots/albums/tracks/views; the artist +
   *    genre tree children are derived client-side from the full track list
   *    (there is no dedicated IPC for them, and the ALL TRACKS node needs
   *    the full list anyway);
   *  · node selection drives the right pane: all → the master list,
   *    artist/album/genre → the matching `library_*` filter query (server
   *    ordering), smart view → `smart_view_run`; a `dataVersion` bump re-runs
   *    the current node after a scan;
   *  · search overrides the pane with debounced (150 ms) `library_search`
   *    results until cleared.
   *
   * The table renders every row but each row opts into
   * `content-visibility: auto` with a fixed 20px intrinsic size, so the
   * browser skips layout/paint for offscreen rows — 100k rows scroll without
   * per-frame work (DOM node count stays O(n); see report notes).
   *
   * Missing backend commands (smart views land in parallel) surface as
   * friendly inline notes via `friendlyIpcError` — never a crash.
   */
  import { open } from "@tauri-apps/plugin-dialog";
  import SmartViewEditor from "./SmartViewEditor.svelte";
  import {
    friendlyIpcError,
    libraryAlbumTracks,
    libraryAlbums,
    libraryRoots,
    libraryScan,
    librarySearch,
    libraryStats,
    libraryTracks,
    onLibraryProgress,
    onLibraryScanned,
    smartViewRun,
    smartViewsList,
    isScanReport,
    type Album,
    type LibraryStats,
    type SavedSmartView,
    type ScanProgressPayload,
    type Track,
  } from "../lib/library/ipc";
  import { emptySmartViewDef } from "../lib/library/smartView";
  import type { QueueItemMeta } from "../lib/player/queue.svelte";

  interface Props {
    /** Append tracks to the play queue. */
    onenqueue: (metas: QueueItemMeta[]) => void;
    /** Append tracks and start playing the first of them. */
    onplaynow: (metas: QueueItemMeta[]) => void;
    onlog: (line: string) => void;
  }

  let { onenqueue, onplaynow, onlog }: Props = $props();

  // Must match the .trow height for content-visibility sizing.
  const ROW_H = 20;

  type Sel =
    | { kind: "all" }
    | { kind: "artist"; value: string }
    | { kind: "album"; value: string }
    | { kind: "genre"; value: string }
    | { kind: "smart"; value: number };

  function selKey(s: Sel): string {
    return s.kind === "all" ? "all" : `${s.kind}:${s.value}`;
  }

  // ── master data (raw: replaced wholesale, never mutated in place) ─────────

  let stats = $state.raw<LibraryStats | null>(null);
  let roots = $state.raw<string[]>([]);
  let albums = $state.raw<Album[]>([]);
  let allTracks = $state.raw<Track[]>([]);
  let views = $state.raw<SavedSmartView[]>([]);
  let tracksError = $state<string | null>(null);
  let viewsError = $state<string | null>(null);
  let statsError = $state<string | null>(null);

  let selection = $state.raw<Sel>({ kind: "all" });
  /** Bumped to force the current node's query to re-run (e.g. after a scan). */
  let dataVersion = $state(0);

  let openGroups = $state({ artists: false, albums: false, genres: false, smart: true });

  // ── node data (artist/album/genre/smart views load on selection) ─────────

  let nodeTracks = $state.raw<Track[] | null>(null);
  let nodeError = $state<string | null>(null);
  let nodeLoading = $state(false);

  $effect(() => {
    // Track selection + dataVersion; re-runs the pane query on either.
    const sel = selection;
    void dataVersion;
    let cancelled = false;
    if (sel.kind === "all") {
      nodeTracks = null;
      nodeError = null;
      nodeLoading = false;
      return () => {
        cancelled = true;
      };
    }
    nodeLoading = true;
    nodeError = null;
    (async () => {
      try {
        let rows: Track[];
        let cmd: string;
        if (sel.kind === "smart") {
          cmd = "smart_view_run";
          rows = await smartViewRun(sel.value);
        } else if (sel.kind === "album") {
          cmd = "library_album_tracks";
          rows = await libraryAlbumTracks(sel.value);
        } else if (sel.kind === "artist") {
          cmd = "library_tracks";
          rows = await libraryTracks({ artist: sel.value });
        } else {
          cmd = "library_tracks";
          rows = await libraryTracks({ genre: sel.value });
        }
        if (cancelled) return;
        nodeTracks = rows;
        nodeError = null;
      } catch (e) {
        if (cancelled) return;
        nodeTracks = [];
        nodeError = friendlyIpcError(
          sel.kind === "smart" ? "this smart view" : "this category",
          e,
        );
        onlog(`media library: ${sel.kind} view error: ${e}`);
      } finally {
        if (!cancelled) nodeLoading = false;
      }
    })();
    return () => {
      cancelled = true;
    };
  });

  // ── search (debounced 150 ms) ──────────────────────────────────────────────

  let searchQ = $state("");
  let searchResults = $state.raw<Track[] | null>(null);
  let searchError = $state<string | null>(null);
  let searchTimer: ReturnType<typeof setTimeout> | null = null;
  let searchSeq = 0;

  function onSearchInput(e: Event & { currentTarget: HTMLInputElement }): void {
    searchQ = e.currentTarget.value;
    if (searchTimer != null) clearTimeout(searchTimer);
    searchTimer = setTimeout(() => void runSearch(searchQ), 150);
  }

  async function runSearch(q: string): Promise<void> {
    const seq = ++searchSeq;
    if (q.trim() === "") {
      searchResults = null;
      searchError = null;
      return;
    }
    try {
      const rows = await librarySearch(q, 500);
      if (seq !== searchSeq) return;
      searchResults = rows;
      searchError = null;
    } catch (e) {
      if (seq !== searchSeq) return;
      searchResults = null;
      searchError = friendlyIpcError("library search", e);
    }
  }

  // ── table selection (plain / ctrl+cmd toggle / shift ranges) ──────────────

  let selectedIds = $state.raw<Set<number>>(new Set());
  let anchorIndex: number | null = null;

  function rowDown(e: PointerEvent, index: number): void {
    if (e.button !== 0) return;
    const id = displayed[index]?.id;
    if (id == null) return;
    if (e.shiftKey && anchorIndex != null) {
      const a = Math.min(anchorIndex, index);
      const b = Math.max(anchorIndex, index);
      const next = new Set<number>();
      for (let i = a; i <= b; i++) next.add(displayed[i].id);
      selectedIds = next;
      return;
    }
    if (e.ctrlKey || e.metaKey) {
      const next = new Set(selectedIds);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      selectedIds = next;
    } else {
      selectedIds = new Set([id]);
    }
    anchorIndex = index;
  }

  function selectAllView(): void {
    selectedIds = new Set(displayed.map((t) => t.id));
  }

  // ── derived tree lists ─────────────────────────────────────────────────────

  const artistNames = $derived.by(() => {
    const set = new Set<string>();
    for (const t of allTracks) if (t.artist !== "") set.add(t.artist);
    return [...set].sort((a, b) => a.localeCompare(b));
  });

  const genreNames = $derived.by(() => {
    const set = new Set<string>();
    for (const t of allTracks) if (t.genre !== "") set.add(t.genre);
    return [...set].sort((a, b) => a.localeCompare(b));
  });

  const displayed = $derived.by(() => {
    if (searchResults != null) return searchResults;
    if (selection.kind === "all") return allTracks;
    return nodeTracks ?? [];
  });

  const selectedMetas = $derived.by(() => {
    if (selectedIds.size === 0) return [];
    return displayed.filter((t) => selectedIds.has(t.id));
  });

  // A blocking message replaces the table only when the current view failed.
  const viewError = $derived.by(() => {
    if (searchResults != null) return null;
    if (selection.kind === "all") return tracksError;
    return nodeError;
  });

  const emptyMessage = $derived.by(() => {
    if (searchResults != null) return `no matches for “${searchQ.trim()}”`;
    if (selection.kind === "all") return "library is empty — use “Scan folder…” above";
    if (selection.kind === "smart") return "this smart view returned no tracks";
    return "no tracks in this category";
  });

  const statsLine = $derived.by(() => {
    if (stats == null) return statsError ?? "…";
    return `${stats.tracks.toLocaleString()} tracks · ${stats.albums.toLocaleString()} albums · ${stats.artists.toLocaleString()} artists`;
  });

  const footerText = $derived.by(() => {
    const n = displayed.length;
    let secs = 0;
    for (const t of displayed) secs += t.durationSecs;
    const scan = scanning
      ? ` · scanning ${progress != null ? `${progress.scanned.toLocaleString()}/${progress.total.toLocaleString()}` : "…"}`
      : "";
    return `${n.toLocaleString()} track${n === 1 ? "" : "s"} · ${fmtLong(secs)}${scan}`;
  });

  // ── smart-view editor ──────────────────────────────────────────────────────

  let editing = $state.raw<{ id: number | null; def: ReturnType<typeof emptySmartViewDef> } | null>(null);

  function newView(): void {
    editing = { id: null, def: emptySmartViewDef() };
  }

  function editView(v: SavedSmartView): void {
    editing = { id: v.id, def: structuredClone(v.def) };
  }

  async function onEditorSave(id: number): Promise<void> {
    editing = null;
    onlog(`media library: smart view #${id} saved`);
    await refreshViews();
    select({ kind: "smart", value: id }); // clears any active search
  }

  async function onEditorDelete(id: number): Promise<void> {
    editing = null;
    onlog(`media library: smart view #${id} deleted`);
    if (selection.kind === "smart" && selection.value === id) selection = { kind: "all" };
    await refreshViews();
  }

  // ── loading ────────────────────────────────────────────────────────────────

  let loadSeq = 0;

  async function refreshAll(): Promise<void> {
    const seq = ++loadSeq;
    const live = <T,>(set: (v: T) => void, err?: (msg: string) => void) => (v: T) => {
      if (seq === loadSeq) set(v);
    };
    // Each facet degrades independently — a failed smart-views list must not
    // take the track table down with it.
    void libraryStats().then(
      live((s) => {
        stats = s;
        statsError = null;
      }),
      (e) => {
        if (seq !== loadSeq) return;
        statsError = friendlyIpcError("library stats", e);
      },
    );
    void libraryRoots().then(live((r) => (roots = r)), () => undefined);
    void libraryAlbums().then(live((a) => (albums = a)), () => undefined);
    void libraryTracks().then(
      live((t) => {
        allTracks = t;
        tracksError = null;
      }),
      (e) => {
        if (seq !== loadSeq) return;
        allTracks = [];
        tracksError = friendlyIpcError("library tracks", e);
        onlog(`media library: tracks error: ${e}`);
      },
    );
    await refreshViews(seq);
  }

  async function refreshViews(seq = ++loadSeq): Promise<void> {
    try {
      const v = await smartViewsList();
      if (seq === loadSeq) {
        views = v;
        viewsError = null;
      }
    } catch (e) {
      if (seq !== loadSeq) return;
      views = [];
      viewsError = friendlyIpcError("smart views", e);
    }
  }

  $effect(() => {
    void refreshAll();
  });

  // ── scanning ───────────────────────────────────────────────────────────────

  let scanning = $state(false);
  let progress = $state.raw<ScanProgressPayload | null>(null);
  let scanError = $state<string | null>(null);

  $effect(() => {
    const up = onLibraryProgress((p) => {
      // Events also fire for scans started elsewhere — latch onto them.
      scanning = true;
      progress = p;
    });
    const us = onLibraryScanned((payload) => {
      scanning = false;
      progress = null;
      if (isScanReport(payload)) {
        scanError = null;
        onlog(
          `library scanned: +${payload.added} ~${payload.updated} −${payload.removed} of ${payload.filesFound} files (${payload.durationMs} ms)`,
        );
      } else {
        scanError = `scan failed: ${payload.error}`;
        onlog(`media library: scan failed: ${payload.error}`);
      }
      void refreshAll();
      dataVersion++; // re-run the current node's query
    });
    return () => {
      void up.then((f) => f());
      void us.then((f) => f());
    };
  });

  async function pickScanFolder(): Promise<void> {
    let dir: string | null = null;
    try {
      dir = await open({ directory: true, multiple: false });
    } catch (e) {
      scanError = `folder picker unavailable: ${e instanceof Error ? e.message : String(e)}`;
      return;
    }
    if (typeof dir !== "string") return;
    await startScan([dir]);
  }

  async function rescan(): Promise<void> {
    if (roots.length === 0) return;
    await startScan(roots);
  }

  async function startScan(dirs: string[]): Promise<void> {
    scanError = null;
    scanning = true;
    progress = null;
    try {
      await libraryScan(dirs);
      // Refresh arrives via the `library-scanned` event (which also covers
      // externally started scans); the resolved report is just logged there.
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      if (/already running/i.test(msg)) {
        scanning = true; // the running scan's events keep the UI live
      } else {
        scanning = false;
        scanError = friendlyIpcError("library scan", e);
        onlog(`media library: scan error: ${msg}`);
      }
    }
  }

  // ── helpers ────────────────────────────────────────────────────────────────

  function select(sel: Sel): void {
    selection = sel;
    // Node selection clears an active search so the pane follows the tree.
    searchQ = "";
    searchResults = null;
    searchError = null;
    if (searchTimer != null) clearTimeout(searchTimer);
  }

  function toMeta(t: Track): QueueItemMeta {
    return {
      path: t.path,
      title: t.title !== "" ? t.title : baseName(t.path),
      artist: t.artist,
      durationSecs: t.durationSecs,
    };
  }

  function toMetas(list: Track[]): QueueItemMeta[] {
    return list.map(toMeta);
  }

  function enqueueAll(): void {
    if (displayed.length > 0) onenqueue(toMetas(displayed));
  }

  function enqueueSelected(): void {
    if (selectedMetas.length > 0) onenqueue(selectedMetas.map(toMeta));
  }

  function playAll(): void {
    if (displayed.length > 0) onplaynow(toMetas(displayed));
  }

  function baseName(path: string): string {
    return path.split(/[\\/]/).pop() ?? path;
  }

  function fmt(secs: number): string {
    const m = Math.floor(secs / 60);
    const s = Math.floor(secs % 60);
    return `${m}:${String(s).padStart(2, "0")}`;
  }

  function fmtLong(secs: number): string {
    const h = Math.floor(secs / 3600);
    const m = Math.floor((secs % 3600) / 60);
    const s = Math.floor(secs % 60);
    return h > 0
      ? `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`
      : `${m}:${String(s).padStart(2, "0")}`;
  }

  function fmtDate(ms: number | null | undefined): string {
    if (ms == null) return "never";
    return new Date(ms).toLocaleString();
  }
</script>

<div class="ml">
  <header class="hdr">MEDIA LIBRARY</header>

  <!-- TASK 3: scan management strip -->
  <div class="scanstrip">
    <button onclick={pickScanFolder} disabled={scanning}>Scan folder…</button>
    {#if roots.length > 0}
      <button onclick={rescan} disabled={scanning} title={roots.join("\n")}>
        Rescan ({roots.length} root{roots.length === 1 ? "" : "s"})
      </button>
    {/if}
    <span class="scan-status">
      {#if scanning}
        scanning {progress != null ? `${progress.scanned.toLocaleString()} / ${progress.total.toLocaleString()}` : "…"}
      {:else if scanError}
        <span class="err">{scanError}</span>
      {:else if stats?.lastScan != null}
        last scan {fmtDate(stats.lastScan)}
      {/if}
    </span>
  </div>

  <!-- top bar: search + actions + stats -->
  <div class="topbar">
    <input
      class="search"
      type="search"
      placeholder="search library…"
      value={searchQ}
      oninput={onSearchInput}
      aria-label="search library" />
    <div class="actions">
      <button onclick={enqueueAll} disabled={displayed.length === 0} title="append every track in view">ENQUEUE ALL</button>
      <button
        onclick={enqueueSelected}
        disabled={selectedMetas.length === 0}
        title="append the selected tracks">ENQUEUE SEL</button>
      <button onclick={playAll} disabled={displayed.length === 0} title="append and start playing">PLAY ALL</button>
    </div>
    <span class="statline" title={roots.join("\n")}>{statsLine}</span>
  </div>
  {#if searchError}
    <div class="search-err">{searchError}</div>
  {/if}

  <div class="body">
    <nav class="tree" aria-label="library categories">
      <button class="node" class:on={selection.kind === "all"} onclick={() => select({ kind: "all" })}>
        ALL TRACKS
      </button>

      <button
        class="node"
        onclick={() => (openGroups.artists = !openGroups.artists)}
        aria-expanded={openGroups.artists}>
        <span class="tw">{openGroups.artists ? "▾" : "▸"}</span>ARTISTS
        <span class="cnt">({artistNames.length})</span>
      </button>
      {#if openGroups.artists}
        {#if artistNames.length === 0 && tracksError == null}
          <div class="leaf empty">no artists</div>
        {/if}
        {#each artistNames as name (name)}
          <button
            class="leaf"
            class:on={selKey(selection) === `artist:${name}`}
            onclick={() => select({ kind: "artist", value: name })}>{name}</button>
        {/each}
      {/if}

      <button
        class="node"
        onclick={() => (openGroups.albums = !openGroups.albums)}
        aria-expanded={openGroups.albums}>
        <span class="tw">{openGroups.albums ? "▾" : "▸"}</span>ALBUMS
        <span class="cnt">({albums.length})</span>
      </button>
      {#if openGroups.albums}
        {#if albums.length === 0 && tracksError == null}
          <div class="leaf empty">no albums</div>
        {/if}
        {#each albums as a (a.albumKey)}
          <button
            class="leaf"
            class:on={selKey(selection) === `album:${a.albumKey}`}
            title={a.artist}
            onclick={() => select({ kind: "album", value: a.albumKey })}
            >{a.name}{a.year != null ? ` (${a.year})` : ""}</button
          >
        {/each}
      {/if}

      <button
        class="node"
        onclick={() => (openGroups.genres = !openGroups.genres)}
        aria-expanded={openGroups.genres}>
        <span class="tw">{openGroups.genres ? "▾" : "▸"}</span>GENRES
        <span class="cnt">({genreNames.length})</span>
      </button>
      {#if openGroups.genres}
        {#if genreNames.length === 0 && tracksError == null}
          <div class="leaf empty">no genres</div>
        {/if}
        {#each genreNames as name (name)}
          <button
            class="leaf"
            class:on={selKey(selection) === `genre:${name}`}
            onclick={() => select({ kind: "genre", value: name })}>{name}</button>
        {/each}
      {/if}

      <button
        class="node"
        onclick={() => (openGroups.smart = !openGroups.smart)}
        aria-expanded={openGroups.smart}>
        <span class="tw">{openGroups.smart ? "▾" : "▸"}</span>SMART VIEWS
        <span class="cnt">({views.length})</span>
      </button>
      {#if openGroups.smart}
        {#each views as v (v.id)}
          <div class="leaf smart" class:on={selKey(selection) === `smart:${v.id}`}>
            <button class="leafbtn" onclick={() => select({ kind: "smart", value: v.id })}>
              {v.def.name}
            </button>
            <button class="edit" title="edit this smart view" onclick={() => editView(v)}>✎</button>
          </div>
        {/each}
        {#if viewsError}
          <div class="leaf empty err" title={viewsError}>{viewsError}</div>
        {/if}
        <button class="leaf new" onclick={newView}>+ NEW SMART VIEW</button>
      {/if}
    </nav>

    <section class="table" aria-label="tracks">
      <div class="thead">
        <span class="c-num">#</span><span class="c-title">TITLE</span
        ><span class="c-artist">ARTIST</span><span class="c-album">ALBUM</span
        ><span class="c-time">TIME</span>
      </div>
      <div class="rows" role="listbox" aria-label="track list">
        {#if viewError}
          <div class="msg err">{viewError}</div>
        {:else if nodeLoading && searchResults == null && selection.kind !== "all"}
          <div class="msg">loading…</div>
        {:else if displayed.length === 0}
          <div class="msg">{emptyMessage}</div>
        {:else}
          {#each displayed as t, i (t.id)}
            <div
              class="trow"
              class:selected={selectedIds.has(t.id)}
              role="option"
              aria-selected={selectedIds.has(t.id)}
              tabindex="-1"
              title={t.path}
              onpointerdown={(e) => rowDown(e, i)}
              ondblclick={() => onplaynow([toMeta(t)])}>
              <span class="c-num">{i + 1}</span>
              <span class="c-title">{t.title !== "" ? t.title : baseName(t.path)}</span>
              <span class="c-artist">{t.artist}</span>
              <span class="c-album">{t.album}</span>
              <span class="c-time">{fmt(t.durationSecs)}</span>
            </div>
          {/each}
        {/if}
      </div>
    </section>
  </div>

  <footer class="bar">
    <span class="totals">{footerText}</span>
    {#if displayed.length > 0 && selectedIds.size > 0}
      <span class="selnote">{selectedIds.size.toLocaleString()} selected</span>
    {/if}
    <button class="selall" onclick={selectAllView} disabled={displayed.length === 0}>SELECT ALL</button>
  </footer>

  {#if editing}
    <SmartViewEditor
      id={editing.id}
      initial={editing.def}
      onsave={(id) => void onEditorSave(id)}
      ondelete={(id) => void onEditorDelete(id)}
      oncancel={() => (editing = null)} />
  {/if}
</div>

<style>
  .ml {
    position: relative; /* hosts the editor overlay */
    width: 640px;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    border: 1px solid #3a3a3a;
    background: #161616;
    user-select: none;
  }

  .hdr {
    padding: 3px 8px;
    font-size: 10px;
    letter-spacing: 0.18em;
    color: #808080;
    background: #101010;
    border-bottom: 1px solid #2a2a2a;
  }

  .scanstrip,
  .topbar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 6px;
    background: #101010;
    border-bottom: 1px solid #2a2a2a;
  }

  .scanstrip button,
  .topbar button,
  .bar button,
  .tree .node,
  .tree .leaf,
  .tree .leafbtn,
  .tree .edit {
    background: #1c1c1c;
    color: #9a9a9a;
    border: 1px solid #3a3a3a;
    border-radius: 2px;
    padding: 1px 6px;
    font-size: 10px;
    letter-spacing: 0.08em;
    cursor: pointer;
  }

  .scanstrip button:hover:not(:disabled),
  .topbar button:hover:not(:disabled),
  .bar button:hover:not(:disabled),
  .tree .node:hover,
  .tree .leaf:hover,
  .tree .leafbtn:hover,
  .tree .edit:hover {
    border-color: #3ccb4e;
    color: #3ccb4e;
  }

  button:disabled {
    opacity: 0.35;
    cursor: default;
  }

  .scan-status {
    margin-left: auto;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 10px;
    color: #707070;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .search {
    flex: 0 1 180px;
    min-width: 80px;
    background: #000;
    color: #3ccb4e;
    border: 1px solid #3a3a3a;
    border-radius: 2px;
    padding: 2px 6px;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 11px;
  }

  .search:focus {
    outline: none;
    border-color: #3ccb4e;
  }

  .actions {
    display: flex;
    gap: 4px;
  }

  .statline {
    margin-left: auto;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 10px;
    color: #707070;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .search-err {
    padding: 2px 8px;
    background: #101010;
    border-bottom: 1px solid #2a2a2a;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 10px;
    color: #d8604c;
  }

  .err {
    color: #d8604c;
  }

  .body {
    height: 320px;
    min-height: 0;
    display: grid;
    grid-template-columns: 176px 1fr;
  }

  /* ── left tree ── */
  .tree {
    overflow-y: auto;
    background: #0d0d0d;
    border-right: 1px solid #2a2a2a;
    padding: 4px;
    display: flex;
    flex-direction: column;
    gap: 1px;
    scrollbar-width: thin;
    scrollbar-color: #2f4f33 #000;
  }

  .tree .node,
  .tree .leaf {
    text-align: left;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    background: transparent;
    border-color: transparent;
    border-radius: 0;
  }

  .tree .node {
    color: #9a9a9a;
    font-weight: 600;
  }

  .tree .leaf {
    padding-left: 20px;
    font-size: 11px;
    color: #3ccb4e;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    content-visibility: auto;
    contain-intrinsic-size: auto 20px;
  }

  .tree .leaf:hover {
    background: #0d0d0d;
    border-color: transparent;
  }

  .tree .leaf.on,
  .tree .node.on {
    color: #8dff9e;
    background: #12301b;
  }

  .tw {
    display: inline-block;
    width: 12px;
    color: #4f4f4f;
  }

  .cnt {
    color: #4f4f4f;
    font-weight: 400;
  }

  .leaf.empty {
    color: #2f4f33;
    cursor: default;
  }

  .leaf.empty:hover {
    color: #2f4f33;
    background: transparent;
    border-color: transparent;
  }

  .leaf.new {
    color: #2f8f3f;
    border: 1px dashed #2f4f33;
    background: transparent;
    margin-top: 2px;
  }

  .leaf.smart {
    display: flex;
    align-items: center;
    gap: 2px;
    padding: 0;
    background: transparent;
    border: none;
  }

  .leaf.smart .leafbtn {
    flex: 1;
    min-width: 0;
    text-align: left;
    border: 1px solid transparent;
    background: transparent;
    color: #3ccb4e;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 11px;
    padding: 1px 4px;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .leaf.smart .edit {
    flex: none;
    border-color: transparent;
    background: transparent;
    color: #4f4f4f;
    padding: 1px 4px;
  }

  .leaf.smart .edit:hover {
    color: #3ccb4e;
  }

  .leaf.smart.on .leafbtn {
    color: #8dff9e;
    background: #12301b;
  }

  /* ── right table ── */
  .table {
    min-width: 0;
    display: flex;
    flex-direction: column;
    background: #000;
  }

  .thead,
  .trow {
    display: grid;
    grid-template-columns: 4ch minmax(8ch, 1.6fr) minmax(6ch, 1fr) minmax(6ch, 1fr) 6ch;
    gap: 6px;
    align-items: center;
    padding: 0 6px;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
  }

  .thead {
    position: sticky;
    top: 0;
    z-index: 1;
    height: 20px;
    line-height: 20px;
    font-size: 10px;
    letter-spacing: 0.1em;
    color: #808080;
    background: #101010;
    border-bottom: 1px solid #2a2a2a;
  }

  .rows {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    scrollbar-width: thin;
    scrollbar-color: #2f4f33 #000;
  }

  .rows::-webkit-scrollbar {
    width: 6px;
  }

  .rows::-webkit-scrollbar-thumb {
    background: #2f4f33;
  }

  .trow {
    height: 20px;
    line-height: 20px;
    font-size: 12px;
    color: #3ccb4e; /* classic Winamp playlist green */
    cursor: default;
    /* Windowed rendering: offscreen rows skip layout/paint but keep their
       reserved 20px, so the scrollbar math stays exact at any list size. */
    content-visibility: auto;
    contain-intrinsic-size: auto 20px;
  }

  .trow:hover {
    background: #0d0d0d;
  }

  .trow.selected {
    background: #12301b;
    color: #6ce87c;
  }

  .c-num {
    opacity: 0.7;
  }

  .c-title,
  .c-artist,
  .c-album {
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .c-artist,
  .c-album {
    color: #2f8f3f;
  }

  .trow.selected .c-artist,
  .trow.selected .c-album {
    color: #58b968;
  }

  .c-time {
    text-align: right;
    opacity: 0.8;
  }

  .msg {
    padding: 6px 8px;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 11px;
    color: #2f4f33;
  }

  /* ── footer ── */
  .bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 3px 6px;
    background: #101010;
    border-top: 1px solid #2a2a2a;
  }

  .totals {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 10px;
    color: #3ccb4e;
  }

  .selnote {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 10px;
    color: #707070;
  }

  .bar .selall {
    margin-left: auto;
  }
</style>
