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
   *    results until cleared;
   *  · NAVIDROME group (M5): SERVER STATUS renders a status card pane
   *    (connection state, live sync progress bar, last report, settings
   *    shortcut) and REMOTE TRACKS lists every `source="navidrome"` row;
   *    a `navidrome-synced` event refreshes stats/tree/pane (dataVersion)
   *    so synced rows appear everywhere at once.
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
    localArtUrl,
    navidromeArtUrl,
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
  import {
    isSyncReport,
    navidromeStar,
    navidromeStatus,
    navidromeSync,
    onNavidromeProgress,
    onNavidromeSynced,
    REMOTE_SOURCE,
    type NavidromeProgressPayload,
    type NavidromeStatus,
    type NavidromeSyncReport,
  } from "../lib/navidrome/ipc";
  import NavidromeSettings from "./NavidromeSettings.svelte";
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
    | { kind: "smart"; value: number }
    /** Navidrome SERVER STATUS card pane (no track list). */
    | { kind: "server" }
    /** All synced `source="navidrome"` rows. */
    | { kind: "remote" };

  function selKey(s: Sel): string {
    return "value" in s ? `${s.kind}:${s.value}` : s.kind;
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

  let openGroups = $state({
    artists: false,
    albums: false,
    genres: false,
    smart: true,
    navidrome: false,
  });

  // ── source filter chip row (ALL / LOCAL / REMOTE) ─────────────────────────
  // Applies to the ARTISTS / ALBUMS / GENRES groups only: tree children are
  // derived from the filtered track list, and pane queries pass `source` to
  // `library_tracks` where supported (plus a client-side re-filter, so an
  // older backend that ignores the unknown arg still shows the right rows).

  type SourceFilter = "all" | "local" | "navidrome";
  let sourceFilter = $state.raw<SourceFilter>("all");
  const SOURCE_CHIPS: { id: SourceFilter; label: string }[] = [
    { id: "all", label: "ALL" },
    { id: "local", label: "LOCAL" },
    { id: "navidrome", label: "REMOTE" },
  ];

  // ── node data (artist/album/genre/smart views load on selection) ─────────

  let nodeTracks = $state.raw<Track[] | null>(null);
  let nodeError = $state<string | null>(null);
  let nodeLoading = $state(false);

  $effect(() => {
    // Track selection + dataVersion + source chip; re-runs the pane query.
    const sel = selection;
    void dataVersion;
    void sourceFilter;
    let cancelled = false;
    if (sel.kind === "all" || sel.kind === "server") {
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
        if (sel.kind === "smart") {
          // Smart views own their `source` rule — the chip is not applied.
          rows = await smartViewRun(sel.value);
        } else if (sel.kind === "album") {
          rows = await libraryAlbumTracks(sel.value); // no source arg → filtered below
        } else if (sel.kind === "remote") {
          rows = await libraryTracks({ source: REMOTE_SOURCE });
        } else if (sel.kind === "artist") {
          rows = await libraryTracks({
            artist: sel.value,
            source: sourceFilter === "all" ? undefined : sourceFilter,
          });
        } else {
          rows = await libraryTracks({
            genre: sel.value,
            source: sourceFilter === "all" ? undefined : sourceFilter,
          });
        }
        if (cancelled) return;
        // Re-apply the chip client-side: an older backend ignores the
        // unknown `source` arg, and album queries never took one.
        if (sel.kind === "artist" || sel.kind === "genre" || sel.kind === "album") {
          rows = rows.filter((t) => sourceFilter === "all" || t.source === sourceFilter);
        }
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

  /** The master list narrowed by the source chip (chip applies to the
   *  ARTISTS/ALBUMS/GENRES groups; ALL TRACKS ignores it). */
  const chipTracks = $derived.by(() => {
    if (sourceFilter === "all") return allTracks;
    return allTracks.filter((t) => t.source === sourceFilter);
  });

  const artistNames = $derived.by(() => {
    const set = new Set<string>();
    for (const t of chipTracks) if (t.artist !== "") set.add(t.artist);
    return [...set].sort((a, b) => a.localeCompare(b));
  });

  const genreNames = $derived.by(() => {
    const set = new Set<string>();
    for (const t of chipTracks) if (t.genre !== "") set.add(t.genre);
    return [...set].sort((a, b) => a.localeCompare(b));
  });

  /** Album keys that survive the chip — cross-referenced from the full
   *  track list because `library_albums` has no source filter. */
  const chipAlbumKeys = $derived.by(() => {
    const set = new Set<string>();
    for (const t of chipTracks) set.add(t.albumKey);
    return set;
  });

  const albumsShown = $derived(
    sourceFilter === "all" ? albums : albums.filter((a) => chipAlbumKeys.has(a.albumKey)),
  );

  /** Synced remote rows, for the REMOTE TRACKS count in the tree. */
  const remoteTrackCount = $derived(
    allTracks.reduce((n, t) => (t.source === REMOTE_SOURCE ? n + 1 : n), 0),
  );

  const displayed = $derived.by(() => {
    if (searchResults != null) return searchResults;
    if (selection.kind === "server") return []; // the pane is the status card
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
    if (selection.kind === "server") return null; // the card owns its errors
    if (selection.kind === "all") return tracksError;
    return nodeError;
  });

  const emptyMessage = $derived.by(() => {
    if (searchResults != null) return `no matches for “${searchQ.trim()}”`;
    if (selection.kind === "all") return "library is empty — use “Scan folder…” above";
    if (selection.kind === "smart") return "this smart view returned no tracks";
    if (selection.kind === "remote") return "no navidrome tracks — connect & sync in SERVER STATUS";
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

  // ── navidrome (M5) ─────────────────────────────────────────────────────────

  let navStatus = $state.raw<NavidromeStatus | null>(null);
  let navError = $state<string | null>(null);
  let navSyncing = $state(false);
  let navProgress = $state.raw<NavidromeProgressPayload | null>(null);
  let navReport = $state.raw<NavidromeSyncReport | null>(null);
  let navSyncError = $state<string | null>(null);
  let showSettings = $state(false);

  /**
   * Starred remote ids — session-local. LIMITATION: no M5 IPC exposes the
   * server's starred state (and Track rows carry no starred flag), so this
   * set starts empty and only grows from clicks here; a later `starred`
   * column / status field should become the source of truth. Rebuilt
   * references (dataVersion bumps after sync) re-read Track rows, which
   * cannot change this set — clicks persist server-side only.
   */
  let starredRemoteIds = $state.raw<Set<string>>(new Set());

  let navSeq = 0;

  async function refreshNavStatus(): Promise<void> {
    const seq = ++navSeq;
    try {
      const s = await navidromeStatus();
      if (seq !== navSeq) return;
      navStatus = s;
      navError = null;
    } catch (e) {
      if (seq !== navSeq) return;
      navStatus = null;
      navError = friendlyIpcError("navidrome", e);
    }
  }

  async function syncNow(): Promise<void> {
    if (navSyncing) return;
    navSyncError = null;
    navSyncing = true;
    navProgress = null;
    try {
      await navidromeSync();
      // Refresh arrives via the `navidrome-synced` event (which also covers
      // externally started syncs); the resolved report is summarized there.
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      if (/already|running/i.test(msg)) {
        navSyncing = true; // the running sync's events keep the UI live
      } else {
        navSyncing = false;
        navSyncError = friendlyIpcError("navidrome sync", e);
        onlog(`navidrome: sync error: ${msg}`);
      }
    }
  }

  async function toggleStar(t: Track): Promise<void> {
    if (t.source !== REMOTE_SOURCE || t.remoteId == null) return;
    const id = t.remoteId;
    const starred = !starredRemoteIds.has(id);
    const next = new Set(starredRemoteIds); // optimistic
    if (starred) next.add(id);
    else next.delete(id);
    starredRemoteIds = next;
    try {
      await navidromeStar(id, starred);
      onlog(`navidrome: ${starred ? "★" : "☆"} ${t.title !== "" ? t.title : id}`);
    } catch (e) {
      const revert = new Set(starredRemoteIds);
      if (starred) revert.delete(id);
      else revert.add(id);
      starredRemoteIds = revert;
      onlog(`media library: star failed: ${e}`);
    }
  }

  $effect(() => {
    void refreshNavStatus();
    const up = onNavidromeProgress((p) => {
      // Events also fire for syncs started elsewhere — latch onto them.
      navSyncing = true;
      navProgress = p;
    });
    const us = onNavidromeSynced((payload) => {
      navSyncing = false;
      navProgress = null;
      if (isSyncReport(payload)) {
        navReport = payload;
        navSyncError = null;
        onlog(
          `navidrome synced: +${payload.tracksAdded} ~${payload.tracksUpdated} −${payload.tracksRemoved} (${payload.durationMs} ms)`,
        );
        void refreshNavStatus();
        void refreshAll();
        dataVersion++; // re-run the current node's query (synced rows landed)
      } else {
        navSyncError = `sync failed: ${payload.error}`;
        onlog(`navidrome: sync failed: ${payload.error}`);
      }
    });
    return () => {
      void up.then((f) => f());
      void us.then((f) => f());
    };
  });

  // ── album cover thumbnails (ALBUMS tree only) ─────────────────────────────
  // Covers appear on album rows — never per track row (the track table
  // renders 100k-row lists; albums are the bounded set). Local art is a
  // pure URL build (the `navinamp-art` scheme serves cached bytes from
  // Rust, immutable-cache headers); navidrome art needs an authed URL per
  // cover id, resolved lazily in small batches the first time the ALBUMS
  // group shows each album. Ids are content hashes / server ids — stable
  // across refreshes — so results cache by id and late replies are
  // harmless. Unresolvable/failed art keeps the ♪ placeholder.

  /** navidrome coverArt id → authed URL (session-local). */
  let remoteArtUrls = $state.raw<Map<string, string>>(new Map());
  /** Art ids whose <img> errored (dead link) → back to the placeholder. */
  let failedArtIds = $state.raw<Set<string>>(new Set());
  /** Ids already queued for `fetchRemoteArt`; each is fetched at most once. */
  const requestedRemoteArt = new Set<string>();

  const REMOTE_ART_BATCH = 8;

  function albumCoverSrc(a: Album): string | null {
    const id = a.artId;
    if (id == null || failedArtIds.has(id)) return null;
    if (a.artSource === REMOTE_SOURCE) return remoteArtUrls.get(id) ?? null;
    if (a.artSource === "local") return localArtUrl(id);
    return null;
  }

  function failArt(id: string): void {
    if (failedArtIds.has(id)) return;
    const next = new Set(failedArtIds);
    next.add(id);
    failedArtIds = next;
  }

  async function fetchRemoteArt(ids: string[]): Promise<void> {
    for (let i = 0; i < ids.length; i += REMOTE_ART_BATCH) {
      const batch = ids.slice(i, i + REMOTE_ART_BATCH);
      const results = await Promise.allSettled(
        batch.map((id) => navidromeArtUrl(id, 64)),
      );
      const next = new Map(remoteArtUrls);
      let changed = false;
      results.forEach((r, k) => {
        if (r.status === "fulfilled") {
          next.set(batch[k], r.value);
          changed = true;
        }
      });
      if (changed) remoteArtUrls = next;
    }
  }

  $effect(() => {
    if (!openGroups.albums) return;
    const wanted: string[] = [];
    for (const a of albumsShown) {
      const id = a.artId;
      if (
        id != null &&
        a.artSource === REMOTE_SOURCE &&
        !remoteArtUrls.has(id) &&
        !failedArtIds.has(id) &&
        !requestedRemoteArt.has(id)
      ) {
        requestedRemoteArt.add(id);
        wanted.push(id);
      }
    }
    if (wanted.length > 0) void fetchRemoteArt(wanted);
  });

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
      // Remote rows keep their navidrome:// path for display, but playback
      // resolves the stream URL through remoteId (queue.feedPath).
      ...(t.source === REMOTE_SOURCE && t.remoteId != null ? { remoteId: t.remoteId } : {}),
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

  function fmtDate(secs: number | null | undefined): string {
    if (secs == null) return "never";
    return new Date(secs * 1000).toLocaleString();
  }
</script>

<div class="ml">
  <header class="hdr">
    <span class="title">MEDIA LIBRARY</span>
    <button class="hdrbtn" onclick={() => (showSettings = true)} title="navidrome connection & streaming">
      NAVIDROME
    </button>
  </header>

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

      <!-- source chip row: filters the ARTISTS / ALBUMS / GENRES groups -->
      <div class="srcrow" role="group" aria-label="source filter">
        {#each SOURCE_CHIPS as c (c.id)}
          <button
            class="chip"
            class:on={sourceFilter === c.id}
            title={`show ${c.label === "REMOTE" ? "navidrome" : c.label.toLowerCase()} tracks in the artists / albums / genres groups`}
            onclick={() => (sourceFilter = c.id)}>{c.label}</button>
        {/each}
      </div>

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
        <span class="cnt">({albumsShown.length})</span>
      </button>
      {#if openGroups.albums}
        {#if albumsShown.length === 0 && tracksError == null}
          <div class="leaf empty">no albums</div>
        {/if}
        {#each albumsShown as a (a.albumKey)}
          <button
            class="leaf album"
            class:on={selKey(selection) === `album:${a.albumKey}`}
            title={a.artist}
            onclick={() => select({ kind: "album", value: a.albumKey })}>
            {#if albumCoverSrc(a)}
              <img
                class="alb-art"
                src={albumCoverSrc(a)}
                alt=""
                loading="lazy"
                decoding="async"
                onerror={() => a.artId != null && failArt(a.artId)} />
            {:else}
              <span class="alb-art none" aria-hidden="true">♪</span>
            {/if}
            <span class="alb-name">{a.name}{a.year != null ? ` (${a.year})` : ""}</span>
          </button>
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

      <!-- M5: navidrome group — server status card + synced remote tracks -->
      <button
        class="node"
        onclick={() => (openGroups.navidrome = !openGroups.navidrome)}
        aria-expanded={openGroups.navidrome}>
        <span class="tw">{openGroups.navidrome ? "▾" : "▸"}</span>NAVIDROME
        {#if navStatus?.configured}
          <span class="cnt on-dot" title="connected">●</span>
        {/if}
      </button>
      {#if openGroups.navidrome}
        <button
          class="leaf"
          class:on={selKey(selection) === "server"}
          onclick={() => select({ kind: "server" })}>SERVER STATUS</button>
        <button
          class="leaf"
          class:on={selKey(selection) === "remote"}
          onclick={() => select({ kind: "remote" })}
          >REMOTE TRACKS <span class="cnt">({remoteTrackCount})</span></button>
        {#if navError}
          <div class="leaf empty err" title={navError}>navidrome unavailable</div>
        {/if}
      {/if}
    </nav>

    {#if selection.kind === "server" && searchResults == null}
      <!-- SERVER STATUS card (search overrides it, like any node) -->
      <section class="server" aria-label="navidrome server status">
        <div class="card">
          <div class="card-h">NAVIDROME SERVER</div>

          {#if navError}
            <p class="card-err">{navError}</p>
          {:else if navStatus == null}
            <p class="dim">loading…</p>
          {:else if !navStatus.configured}
            <p class="state">○ NOT CONNECTED</p>
            <p class="dim">connect a server first — open SETTINGS… below</p>
          {:else}
            <p class="state ok">● CONNECTED</p>
            <div class="kv"><span>url</span><b>{navStatus.url}</b></div>
            <div class="kv"><span>user</span><b>{navStatus.user}</b></div>
            <div class="kv"><span>version</span><b>{navStatus.serverVersion ?? "—"}</b></div>
            <div class="kv"><span>tracks</span><b>{navStatus.trackCount.toLocaleString()}</b></div>
            <div class="kv"><span>last sync</span><b>{fmtDate(navStatus.syncedAt)}</b></div>
          {/if}

          {#if navReport}
            <div class="report" title={navReport.errors.join("\n")}>
              last sync: +{navReport.tracksAdded} added · ~{navReport.tracksUpdated} updated ·
              −{navReport.tracksRemoved} removed
              {#if navReport.errors.length > 0}
                · {navReport.errors.length} error{navReport.errors.length === 1 ? "" : "s"}
              {/if}
              ({navReport.durationMs} ms)
            </div>
          {/if}

          {#if navSyncing}
            <div class="sync">
              {#if navProgress != null && navProgress.total > 0}
                <div class="pbar-track">
                  <div
                    class="pbar-fill"
                    style={`width:${Math.min(100, (navProgress.scanned / navProgress.total) * 100).toFixed(1)}%`}>
                  </div>
                </div>
                <span class="plabel">
                  {navProgress.scanned.toLocaleString()} / {navProgress.total.toLocaleString()}
                </span>
              {:else}
                <span class="plabel">syncing…</span>
              {/if}
            </div>
          {:else if navSyncError}
            <p class="card-err">{navSyncError}</p>
          {/if}

          <div class="card-actions">
            <button
              onclick={syncNow}
              disabled={navSyncing || navStatus?.configured !== true}
              title="full library sync">{navSyncing ? "SYNCING…" : "SYNC NOW"}</button>
            <button onclick={() => (showSettings = true)} title="connection & streaming settings">
              SETTINGS…
            </button>
          </div>
        </div>
      </section>
    {:else}
      <section class="table" aria-label="tracks">
        <div class="thead">
          <span class="c-num">#</span><span class="c-title">TITLE</span
          ><span class="c-artist">ARTIST</span><span class="c-album">ALBUM</span
          ><span class="c-time">TIME</span><span class="c-star" title="star (navidrome)">★</span>
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
                <span class="c-title">{#if t.source === REMOTE_SOURCE}<span class="rmark" title="navidrome track">◈ </span>{/if}{t.title !== "" ? t.title : baseName(t.path)}</span>
                <span class="c-artist">{t.artist}</span>
                <span class="c-album">{t.album}</span>
                <span class="c-time">{fmt(t.durationSecs)}</span>
                <span class="c-star">
                  {#if t.source === REMOTE_SOURCE && t.remoteId != null}
                    <button
                      class="star"
                      class:on={starredRemoteIds.has(t.remoteId)}
                      title={starredRemoteIds.has(t.remoteId) ? "unstar on server" : "star on server"}
                      onpointerdown={(e) => e.stopPropagation()}
                      ondblclick={(e) => e.stopPropagation()}
                      onclick={() => void toggleStar(t)}>
                      {starredRemoteIds.has(t.remoteId) ? "★" : "☆"}
                    </button>
                  {/if}
                </span>
              </div>
            {/each}
          {/if}
        </div>
      </section>
    {/if}
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

  {#if showSettings}
    <NavidromeSettings
      onclose={() => (showSettings = false)}
      onchanged={() => void refreshNavStatus()}
      onlog={onlog} />
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
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 3px 8px;
    font-size: 10px;
    letter-spacing: 0.18em;
    color: #808080;
    background: #101010;
    border-bottom: 1px solid #2a2a2a;
  }

  .hdr .title {
    flex: 1;
  }

  .hdrbtn {
    background: #1c1c1c;
    color: #9a9a9a;
    border: 1px solid #3a3a3a;
    border-radius: 2px;
    padding: 1px 6px;
    font-size: 10px;
    letter-spacing: 0.08em;
    cursor: pointer;
  }

  .hdrbtn:hover {
    border-color: #3ccb4e;
    color: #3ccb4e;
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

  /* ── album covers (ALBUMS tree leaves only) ── */
  .tree .leaf.album {
    display: flex;
    align-items: center;
    gap: 5px;
    height: 28px;
    line-height: normal;
    contain-intrinsic-size: auto 28px; /* taller than plain leaves */
  }

  .tree .leaf.album .alb-name {
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .alb-art {
    flex: none;
    width: 28px;
    height: 28px;
    object-fit: cover;
    border: 1px solid #2a2a2a;
  }

  /* No art (or a failed load): the classic green note placeholder. */
  .alb-art.none {
    display: grid;
    place-items: center;
    color: #2f8f3f;
    background: #0d1a10;
    font-size: 13px;
  }

  .cnt.on-dot {
    color: #3ccb4e;
    font-weight: 400;
  }

  /* ── source chip row ── */
  .srcrow {
    display: flex;
    gap: 2px;
    padding: 2px 0 2px 14px;
  }

  .srcrow .chip {
    flex: 1;
    background: transparent;
    border: 1px solid #2a2a2a;
    border-radius: 2px;
    padding: 0;
    color: #4f4f4f;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 9px;
    letter-spacing: 0.06em;
    cursor: pointer;
  }

  .srcrow .chip:hover {
    color: #3ccb4e;
    border-color: #2f4f33;
  }

  .srcrow .chip.on {
    color: #3ccb4e;
    border-color: #3ccb4e;
    background: #0d1a10;
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
    grid-template-columns: 4ch minmax(8ch, 1.6fr) minmax(6ch, 1fr) minmax(6ch, 1fr) 6ch 14px;
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

  .c-star {
    text-align: center;
  }

  .thead .c-star {
    color: #4f4f4f;
    font-size: 10px;
  }

  .rmark {
    color: #3a7a4a;
  }

  .trow .star {
    background: none;
    border: none;
    padding: 0;
    color: #3a5a3f;
    font-size: 11px;
    line-height: 1;
    cursor: pointer;
  }

  .trow .star:hover {
    color: #3ccb4e;
  }

  .trow .star.on {
    color: #d8a63c;
  }

  .msg {
    padding: 6px 8px;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 11px;
    color: #2f4f33;
  }

  /* ── navidrome server status card ── */
  .server {
    min-width: 0;
    overflow-y: auto;
    background: #000;
    scrollbar-width: thin;
    scrollbar-color: #2f4f33 #000;
  }

  .card {
    display: flex;
    flex-direction: column;
    gap: 8px;
    width: 100%;
    max-width: 420px;
    padding: 12px;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
  }

  .card-h {
    font-size: 10px;
    letter-spacing: 0.15em;
    color: #707070;
    border-bottom: 1px solid #2a2a2a;
    padding-bottom: 4px;
  }

  .card p {
    margin: 0;
  }

  .state {
    font-size: 11px;
    letter-spacing: 0.1em;
    color: #808080;
  }

  .state.ok {
    color: #3ccb4e;
  }

  .dim {
    font-size: 10px;
    color: #4f4f4f;
  }

  .card-err {
    font-size: 11px;
    color: #d8604c;
  }

  .kv {
    display: grid;
    grid-template-columns: 10ch 1fr;
    gap: 6px;
    align-items: baseline;
    font-size: 11px;
    color: #3ccb4e;
  }

  .kv span {
    color: #4f4f4f;
  }

  .kv b {
    font-weight: 400;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .report {
    font-size: 10px;
    color: #2f8f3f;
    border: 1px solid #2a2a2a;
    background: #101010;
    padding: 4px 6px;
  }

  .sync {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .pbar-track {
    height: 8px;
    border: 1px solid #3a3a3a;
    background: #0d0d0d;
  }

  .pbar-fill {
    height: 100%;
    background: #3ccb4e;
    transition: width 0.2s linear;
  }

  .plabel {
    font-size: 10px;
    color: #707070;
  }

  .card-actions {
    display: flex;
    gap: 6px;
  }

  .card-actions button {
    background: #1c1c1c;
    color: #9a9a9a;
    border: 1px solid #3a3a3a;
    border-radius: 2px;
    padding: 1px 6px;
    font-size: 10px;
    letter-spacing: 0.08em;
    cursor: pointer;
  }

  .card-actions button:hover:not(:disabled) {
    border-color: #3ccb4e;
    color: #3ccb4e;
  }

  .card-actions button:disabled {
    opacity: 0.35;
    cursor: default;
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
