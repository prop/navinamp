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
   *  · startup loads only stats/roots/saved views; artist / album / genre
   *    lists are fetched lazily when a group expands, and the track table
   *    pages 500 rows on entry (infinite scroll) — there is no full-library
   *    snapshot;
   *  · node selection drives the right pane: all → the paged master list,
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
   * Large lists are windowed: the track table renders only the visible rows
   * (+ overscan), and ARTISTS / ALBUMS / GENRES keep their full scroll
   * height while rendering just the visible leaf window. The tree's other
   * rows (group headers, smart views, playlists) are small and always render.
   *
   * Missing backend commands (smart views land in parallel) surface as
   * friendly inline notes via `friendlyIpcError` — never a crash.
   */
  import { open } from "@tauri-apps/plugin-dialog";
  import ContextMenu, { type ContextMenuItem } from "./ContextMenu.svelte";
  import SmartViewEditor from "./SmartViewEditor.svelte";
  import {
    clearEqFor,
    getArtistEq,
    getTrackEq,
    rememberEq,
  } from "../lib/audio/auto";
  import {
    friendlyIpcError,
    libraryAlbumTracks,
    libraryAlbums,
    libraryArtists,
    libraryGenres,
    libraryRoots,
    libraryScan,
    librarySearch,
    librarySetRating,
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
    type SourceStats,
    type Track,
  } from "../lib/library/ipc";
  import { emptySmartViewDef } from "../lib/library/smartView";
  import {
    isSyncReport,
    navidromePlaylistTracks,
    navidromePlaylists,
    navidromeProfiles,
    navidromePublishSmartView,
    navidromeRate,
    navidromeStar,
    navidromeStatus,
    navidromeSync,
    onNavidromeProgress,
    onNavidromeSynced,
    REMOTE_SOURCE,
    type NavidromePlaylist,
    type NavidromeProfile,
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
    | { kind: "remote" }
    /** One saved server's synced rows (value = profile id). */
    | { kind: "remoteServer"; value: string }
    /** One server playlist, fetched live (value = Subsonic playlist id). */
    | { kind: "playlist"; value: string; server: string };

  function selKey(s: Sel): string {
    if (s.kind === "playlist") return `${s.kind}:${s.server}:${s.value}`;
    return "value" in s ? `${s.kind}:${s.value}` : s.kind;
  }

  // ── small master data / lazily fetched tree lists ────────────────────────
  // The library no longer holds a 20k-row master snapshot. Stats are counts;
  // the table pages the current view; tree lists load when a group expands.

  let stats = $state.raw<LibraryStats | null>(null);
  let roots = $state.raw<string[]>([]);
  let views = $state.raw<SavedSmartView[]>([]);
  let albums = $state.raw<Album[]>([]);
  let artistNames = $state.raw<string[]>([]);
  let genreNames = $state.raw<string[]>([]);
  let treeError = $state<string | null>(null);
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
  /** Last queried node (selection + chip); distinguishes a real node change
   *  from a background re-run (scan/sync progress) so rows don't blank. */
  let nodeQueryKey = "";

  $effect(() => {
    // Track selection + dataVersion + source chip; re-runs the pane query.
    const sel = selection;
    void dataVersion;
    void sourceFilter;
    const key = `${selKey(sel)}::${sourceFilter}`;
    const changed = key !== nodeQueryKey;
    nodeQueryKey = key;
    let cancelled = false;
    if (
      sel.kind === "all" ||
      sel.kind === "server" ||
      sel.kind === "remote" ||
      sel.kind === "remoteServer"
    ) {
      // ALL / REMOTE are paged views (their own effect); only filtered node
      // queries fetch their rows here.
      // filtered node queries fetch their own rows.
      nodeTracks = null;
      nodeError = null;
      nodeLoading = false;
      return () => {
        cancelled = true;
      };
    }
    // A new node starts from "loading…"; a background refresh keeps the
    // current rows visible until the new result lands.
    if (changed) nodeTracks = null;
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
        } else if (sel.kind === "playlist") {
          // Ephemeral rows straight from the server — never imported.
          rows = await navidromePlaylistTracks(sel.server, sel.value);
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
  /**
   * SELECT ALL is virtual for paged views: it covers every row matching the
   * current view (not just the loaded pages), and actions fetch the rows on
   * demand. Any explicit row click leaves this mode.
   */
  let selectAllMode = $state(false);

  function isRowSelected(t: Track): boolean {
    return selectAllMode || selectedIds.has(t.id);
  }

  function rowDown(e: PointerEvent, index: number): void {
    if (e.button !== 0) return;
    const id = displayed[index]?.id;
    if (id == null) return;
    selectAllMode = false;
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
    if (selectAllMode) {
      selectAllMode = false;
      selectedIds = new Set();
      anchorIndex = null;
      return;
    }
    // No ids are materialized: the view may hold 20k+ rows across pages that
    // have not been fetched. Bulk actions call `allRowsForAction()` instead.
    selectedIds = new Set();
    selectAllMode = true;
    anchorIndex = null;
  }

  // ── paged table (ALL TRACKS / REMOTE TRACKS) ─────────────────────────────
  // Rows are loaded lazily: a first page on view entry and one page at a
  // time as the user scrolls. The scrollbar length comes from the stats
  // counts, so it stays honest before every row is fetched.

  const PAGE_SIZE = 500;

  let pagedRows = $state.raw<Track[]>([]);
  let pageOffset = 0;
  let pageLoading = $state(false);
  let pageError = $state<string | null>(null);
  let pageSeq = 0;
  let rowsEl = $state<HTMLElement | null>(null);
  /** Vertical scroll offset of the track list (virtualization window). */
  let scrollTop = $state(0);

  const pagedView = $derived(
    selection.kind === "all" ||
      selection.kind === "remote" ||
      selection.kind === "remoteServer",
  );

  const displayedTotal = $derived.by(() => {
    if (searchResults != null) return searchResults.length;
    if (selection.kind === "server") return 0;
    if (selection.kind === "all") return stats?.tracks ?? 0;
    if (selection.kind === "remote") return stats?.navidrome.tracks ?? 0;
    if (selection.kind === "remoteServer") {
      const serverId = selection.value;
      return (
        stats?.navidromeServers?.find((s) => s.serverId === serverId)?.tracks ?? 0
      );
    }
    return nodeTracks?.length ?? 0;
  });

  async function loadPaged(reset: boolean): Promise<void> {
    if (!pagedView) return;
    if (!reset && (pageLoading || pagedRows.length >= displayedTotal)) return;
    const seq = ++pageSeq;
    if (reset) {
      pageOffset = 0;
      pagedRows = [];
      pageError = null;
      if (rowsEl) rowsEl.scrollTop = 0;
    }
    pageLoading = true;
    try {
      const rows = await libraryTracks({
        source:
          selection.kind === "remote" || selection.kind === "remoteServer"
            ? REMOTE_SOURCE
            : undefined,
        serverId: selection.kind === "remoteServer" ? selection.value : undefined,
        limit: PAGE_SIZE,
        offset: pageOffset,
      });
      if (seq !== pageSeq) return;
      pagedRows = reset ? rows : [...pagedRows, ...rows];
      pageOffset += PAGE_SIZE;
      pageError = null;
    } catch (e) {
      if (seq !== pageSeq) return;
      pageError = friendlyIpcError("library tracks", e);
      onlog(`media library: page error: ${e}`);
    } finally {
      if (seq === pageSeq) pageLoading = false;
    }
  }

  // New view or a data refresh (scan/sync completion): restart at page 1.
  $effect(() => {
    const sel = selection;
    void dataVersion;
    if (sel.kind !== "all" && sel.kind !== "remote" && sel.kind !== "remoteServer") return;
    void loadPaged(true);
    return () => {
      pageSeq++; // cancel an in-flight page when the view changes
    };
  });

  /** Infinite scroll: fetch the next page near the bottom of the list. */
  function onRowsScroll(e: Event & { currentTarget: HTMLElement }): void {
    const el = e.currentTarget;
    scrollTop = el.scrollTop;
    if (!pagedView) return;
    if (searchResults != null) return; // search results are client-side
    if (el.scrollTop + el.clientHeight >= el.scrollHeight - 240) {
      void loadPaged(false);
    }
  }

  // ── tree lists (lazy: fetched when a group expands / the chip changes) ───

  let artistsSeq = 0;
  let genresSeq = 0;
  let albumsSeq = 0;

  function chipSource(): string | undefined {
    return sourceFilter === "all" ? undefined : sourceFilter;
  }

  async function loadArtists(): Promise<void> {
    const seq = ++artistsSeq;
    try {
      const names = await libraryArtists(chipSource());
      if (seq !== artistsSeq) return;
      artistNames = names;
      treeError = null;
    } catch (e) {
      if (seq !== artistsSeq) return;
      artistNames = [];
      treeError = friendlyIpcError("artists", e);
    }
  }

  async function loadGenres(): Promise<void> {
    const seq = ++genresSeq;
    try {
      const names = await libraryGenres(chipSource());
      if (seq !== genresSeq) return;
      genreNames = names;
      treeError = null;
    } catch (e) {
      if (seq !== genresSeq) return;
      genreNames = [];
      treeError = friendlyIpcError("genres", e);
    }
  }

  async function loadAlbums(): Promise<void> {
    const seq = ++albumsSeq;
    try {
      const list = await libraryAlbums(chipSource());
      if (seq !== albumsSeq) return;
      albums = list;
      treeError = null;
    } catch (e) {
      if (seq !== albumsSeq) return;
      albums = [];
      treeError = friendlyIpcError("albums", e);
    }
  }

  $effect(() => {
    if (!openGroups.artists) return;
    void sourceFilter;
    void dataVersion;
    void loadArtists();
  });

  $effect(() => {
    if (!openGroups.genres) return;
    void sourceFilter;
    void dataVersion;
    void loadGenres();
  });

  $effect(() => {
    if (!openGroups.albums) return;
    void sourceFilter;
    void dataVersion;
    void loadAlbums();
  });

  /** Tree counts come from the cheap stats query, not from loaded lists. */
  const chipStats = $derived.by((): SourceStats | null => {
    if (!stats) return null;
    if (sourceFilter === "all") return stats;
    return sourceFilter === "local" ? stats.local : stats.navidrome;
  });

  /** Virtual tree rows: only the rows around the viewport are in the DOM.
   *  Slot heights include the tree's 1px gap so offset math stays exact
   *  (leaf row 19 + 1, album row 28 + 1). Counts still come from stats. */
  const TREE_LEAF_H = 20;
  const TREE_ALBUM_H = 29;
  const TREE_OVERSCAN = 8;
  /** A–Z jump rails appear once a list is long enough to need them. */
  const TREE_JUMP_MIN = 30;
  const TREE_JUMP_H = 24;

  interface RowWindow {
    start: number;
    end: number;
  }

  let treeEl = $state<HTMLElement | null>(null);
  let artistsListEl = $state<HTMLElement | null>(null);
  let albumsListEl = $state<HTMLElement | null>(null);
  let genresListEl = $state<HTMLElement | null>(null);
  let artistWindow = $state.raw<RowWindow>({ start: 0, end: 0 });
  let albumWindow = $state.raw<RowWindow>({ start: 0, end: 0 });
  let genreWindow = $state.raw<RowWindow>({ start: 0, end: 0 });

  /** Sticky rail height for a list that is (or is not) long enough for one. */
  function railHeight(count: number): number {
    return count >= TREE_JUMP_MIN ? TREE_JUMP_H : 0;
  }

  /** Virtual container height: all row slots plus the in-flow rail. */
  function listHeight(count: number, rowH: number): number {
    return Math.max(0, count * rowH - 1) + railHeight(count);
  }

  function treeWindow(
    el: HTMLElement | null,
    count: number,
    slotH: number,
    topOffset: number,
    scrollTop: number,
    viewport: number,
  ): RowWindow {
    if (el == null || count === 0) return { start: 0, end: 0 };
    // offsetTop is measured from the tree's padding edge, the same origin
    // scrollTop uses. Rows start below the sticky rail; the ±4px padding
    // error is absorbed by the overscan.
    const relative = scrollTop - el.offsetTop - topOffset;
    const start = Math.max(0, Math.floor(relative / slotH) - TREE_OVERSCAN);
    const end = Math.min(
      count,
      Math.max(start, Math.ceil((relative + viewport) / slotH) + TREE_OVERSCAN),
    );
    return { start, end };
  }

  function updateTreeWindows(): void {
    const scrollTop = treeEl?.scrollTop ?? 0;
    const viewport = treeEl?.clientHeight ?? 0;

    const nextArtists = treeWindow(
      artistsListEl,
      artistNames.length,
      TREE_LEAF_H,
      railHeight(artistNames.length),
      scrollTop,
      viewport,
    );
    if (nextArtists.start !== artistWindow.start || nextArtists.end !== artistWindow.end) {
      artistWindow = nextArtists;
    }

    const nextAlbums = treeWindow(
      albumsListEl,
      albums.length,
      TREE_ALBUM_H,
      railHeight(albums.length),
      scrollTop,
      viewport,
    );
    if (nextAlbums.start !== albumWindow.start || nextAlbums.end !== albumWindow.end) {
      albumWindow = nextAlbums;
    }

    const nextGenres = treeWindow(
      genresListEl,
      genreNames.length,
      TREE_LEAF_H,
      railHeight(genreNames.length),
      scrollTop,
      viewport,
    );
    if (nextGenres.start !== genreWindow.start || nextGenres.end !== genreWindow.end) {
      genreWindow = nextGenres;
    }
  }

  const artistsWindowed = $derived(
    artistNames.slice(artistWindow.start, artistWindow.end),
  );
  /** Albums are fetched pre-filtered by the source chip. */
  const albumsWindowed = $derived(albums.slice(albumWindow.start, albumWindow.end));
  const genresWindowed = $derived(genreNames.slice(genreWindow.start, genreWindow.end));

  // Groups that get an A–Z rail.
  type JumpGroup = "artists" | "albums" | "genres";

  function initialOf(text: string): string {
    const first = Array.from(text.trim())[0];
    if (first === undefined) return "#";
    // Fold accents (É → E) and collapse digits/symbols/other scripts into a
    // single "#" bucket so the rail never needs more than 27 cells.
    const base = first.normalize("NFD").replace(/\p{M}/gu, "").toUpperCase();
    return /^[A-Z]$/.test(base) ? base : "#";
  }

  function initialsOf(items: string[]): string[] {
    const seen = new Set<string>();
    const out: string[] = [];
    for (const item of items) {
      const initial = initialOf(item);
      if (seen.has(initial)) continue;
      seen.add(initial);
      out.push(initial);
    }
    return out;
  }

  function initialAt(items: string[], index: number): string {
    const item = items[index];
    return item === undefined ? "" : initialOf(item);
  }

  // Keys follow each list's own order: albums are sorted by artist, so their
  // rail uses the artist initial (not the album title).
  const albumJumpKeys = $derived(albums.map((a) => a.artist || a.name));
  const artistInitials = $derived(initialsOf(artistNames));
  const albumInitials = $derived(initialsOf(albumJumpKeys));
  const genreInitials = $derived(initialsOf(genreNames));

  // Highlight the initial at the top of the actual viewport (the window
  // starts TREE_OVERSCAN rows above it, except at the very top).
  function visibleStart(win: RowWindow, count: number): number {
    if (count === 0) return 0;
    const index = win.start === 0 ? 0 : win.start + TREE_OVERSCAN;
    return Math.min(index, count - 1);
  }

  const artistJumpLetter = $derived(
    initialAt(artistNames, visibleStart(artistWindow, artistNames.length)),
  );
  const albumJumpLetter = $derived(
    initialAt(albumJumpKeys, visibleStart(albumWindow, albumJumpKeys.length)),
  );
  const genreJumpLetter = $derived(
    initialAt(genreNames, visibleStart(genreWindow, genreNames.length)),
  );

  function jumpTo(group: JumpGroup, letter: string): void {
    const items =
      group === "artists"
        ? artistNames
        : group === "albums"
          ? albumJumpKeys
          : genreNames;
    const index = items.findIndex((item) => initialOf(item) === letter);
    const listEl =
      group === "artists" ? artistsListEl : group === "albums" ? albumsListEl : genresListEl;
    if (index < 0 || listEl == null || treeEl == null) return;
    const rowH = group === "albums" ? TREE_ALBUM_H : TREE_LEAF_H;
    // The sticky rail occupies the top of the scrollport; the row below it
    // lands at y = railHeight + 4 (or 4 when the list has no rail).
    treeEl.scrollTop = Math.max(0, listEl.offsetTop + index * rowH - 4);
    updateTreeWindows();
  }

  // Recompute after the tree DOM changes: a group toggle/list load shifts
  // every list below it, and the refs attach/detach with the group.
  $effect(() => {
    void openGroups.artists;
    void openGroups.albums;
    void openGroups.genres;
    void sourceFilter;
    void artistNames;
    void albums;
    void genreNames;
    void treeEl;
    void artistsListEl;
    void albumsListEl;
    void genresListEl;
    updateTreeWindows();
  });

  function onTreeScroll(): void {
    updateTreeWindows();
  }

  // ── current table view ─────────────────────────────────────────────────────

  const displayed = $derived.by(() => {
    if (searchResults != null) return searchResults;
    if (selection.kind === "server") return []; // the pane is the status card
    if (pagedView) return pagedRows;
    return nodeTracks ?? [];
  });

  // Row virtualization: only the visible slice (plus overscan) is rendered.
  // Spacers above/below preserve the true scroll height for the full view.
  const ROWS_VIEWPORT = 300; // .body height 320 − thead 20
  const OVERSCAN = 8;
  const windowStart = $derived(Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN));
  const windowEnd = $derived(
    Math.min(displayed.length, Math.ceil((scrollTop + ROWS_VIEWPORT) / ROW_H) + OVERSCAN),
  );
  const windowRows = $derived(displayed.slice(windowStart, windowEnd));

  // Any view/search change starts back at the top of the list and leaves the
  // virtual SELECT ALL mode (ids are view-relative).
  $effect(() => {
    void selection;
    void searchResults;
    scrollTop = 0;
    selectAllMode = false;
    if (rowsEl) rowsEl.scrollTop = 0;
  });

  const tableLoading = $derived(
    searchResults == null &&
      (pagedView
        ? pageLoading && pagedRows.length === 0
        : nodeLoading && nodeTracks == null && selection.kind !== "server"),
  );

  const selectedMetas = $derived.by(() => {
    if (selectedIds.size === 0) return [];
    return displayed.filter((t) => selectedIds.has(t.id));
  });

  /** Rows covered by the current selection (virtual SELECT ALL included). */
  const selectedCount = $derived(selectAllMode ? displayedTotal : selectedMetas.length);

  // A blocking message replaces the table only when the current view failed.
  const viewError = $derived.by(() => {
    if (searchResults != null || selection.kind === "server") return null;
    if (pagedView) return pageError;
    return nodeError;
  });

  const emptyMessage = $derived.by(() => {
    if (searchResults != null) return `no matches for “${searchQ.trim()}”`;
    if (selection.kind === "all") return "library is empty — use “Scan folder…” above";
    if (selection.kind === "smart") return "this smart view returned no tracks";
    if (selection.kind === "remote") return "no navidrome tracks — sync from SERVER STATUS";
    if (selection.kind === "remoteServer") {
      return "no tracks from this server yet — sync from SERVER STATUS";
    }
    if (selection.kind === "playlist") return "this server playlist is empty";
    return "no tracks in this category";
  });

  const statsLine = $derived.by(() => {
    if (stats == null) return statsError ?? "…";
    return `${stats.tracks.toLocaleString()} tracks · ${stats.albums.toLocaleString()} albums · ${stats.artists.toLocaleString()} artists`;
  });

  const footerText = $derived.by(() => {
    const paged = pagedView;
    const n = paged ? displayedTotal : displayed.length;
    let secs = 0;
    if (paged) {
      if (selection.kind === "remoteServer") {
        const serverId = selection.value;
        secs =
          stats?.navidromeServers?.find((s) => s.serverId === serverId)?.totalSecs ?? 0;
      } else if (selection.kind === "remote") {
        secs = stats?.navidrome.totalSecs ?? 0;
      } else {
        secs = stats?.totalSecs ?? 0;
      }
    } else {
      for (const t of displayed) secs += t.durationSecs;
    }
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

  /** Publish a saved smart view as a Navidrome playlist (editor button). */
  async function onEditorPublish(id: number, serverId: string): Promise<string> {
    const report = await navidromePublishSmartView(id, serverId);
    const serverName = navProfiles.find((p) => p.id === serverId)?.name ?? serverId;
    onlog(
      `navidrome[${serverName}]: ${report.created ? "created" : "updated"} playlist ` +
        `"${report.name}" (${report.trackCount} track${report.trackCount === 1 ? "" : "s"})`,
    );
    // The target server's playlist list (NAVIDROME → PLAYLISTS) may have a
    // new entry.
    void refreshPlaylists(serverId);
    void refreshViews(); // publishing saves the edited name/rules
    return `${report.created ? "Published" : "Updated"} “${report.name}” — ` +
      `${report.trackCount} track${report.trackCount === 1 ? "" : "s"} on ${serverName}`;
  }

  // ── loading ────────────────────────────────────────────────────────────────

  let loadSeq = 0;

  async function refreshAll(): Promise<void> {
    const seq = ++loadSeq;
    const live = <T,>(set: (v: T) => void) => (v: T) => {
      if (seq === loadSeq) set(v);
    };
    // Startup only loads cheap counts/metadata. The table pages itself and
    // tree lists load when a group expands — no 20k-row master snapshot.
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

  // ── sync-time refresh ─────────────────────────────────────────────────────
  // Counts update continuously; the table pages and tree lists are small
  // lazy queries, so there is no 20k-row aggregate reload anymore.

  let statsSeq = 0;

  function refreshStats(): void {
    const seq = ++statsSeq;
    void libraryStats().then(
      (s) => {
        if (seq !== statsSeq) return;
        stats = s;
        statsError = null;
      },
      () => undefined, // keep the previous snapshot; refreshAll is final
    );
  }

  /** Re-fetch only the tree lists the user currently has open. */
  function refreshTreeLists(): void {
    if (openGroups.artists) void loadArtists();
    if (openGroups.genres) void loadGenres();
    if (openGroups.albums) void loadAlbums();
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

  /** Synced remote rows, for the REMOTE TRACKS count in the tree (cheap
   *  stats query; advances during a sync as batches land). */
  const remoteTrackCount = $derived(stats?.navidrome.tracks ?? 0);
  let navError = $state<string | null>(null);
  let navSyncing = $state(false);
  let navProgress = $state.raw<NavidromeProgressPayload | null>(null);
  let navReport = $state.raw<NavidromeSyncReport | null>(null);
  let navSyncError = $state<string | null>(null);
  let showSettings = $state(false);

  /**
   * Optimistic star overrides for remote tracks, keyed by remote id. Rows
   * carry the persisted `starred` flag (synced from the server, mirrored on
   * every star click); overrides only cover the window between the click and
   * the next sync/refresh.
   */
  let starOverrides = $state.raw<Map<string, boolean>>(new Map());

  function starKey(serverId: string | null, remoteId: string): string {
    return `${serverId ?? ""}/${remoteId}`;
  }

  function isStarred(t: Track): boolean {
    if (t.remoteId == null) return false;
    return starOverrides.get(starKey(t.serverId, t.remoteId)) ?? t.starred;
  }

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

  /** Saved server profiles (labels for the per-server tree leaves). */
  let navProfiles = $state.raw<NavidromeProfile[]>([]);

  async function refreshNavProfiles(): Promise<void> {
    try {
      navProfiles = await navidromeProfiles();
    } catch {
      navProfiles = []; // tree falls back to server ids
    }
  }

  // A removed server's filtered view would otherwise stay empty forever.
  $effect(() => {
    const selectedServer = selection.kind === "remoteServer" ? selection.value : null;
    if (
      selectedServer != null &&
      navProfiles.length > 0 &&
      !navProfiles.some((p) => p.id === selectedServer)
    ) {
      selection = { kind: "remote" };
    }
  });

  // Server playlists (browse-only): one live list per saved server, fetched
  // when the NAVIDROME group is open. Opening one queries that server live
  // via `getPlaylist`; nothing is persisted.
  interface ServerPlaylists {
    serverId: string;
    name: string;
    playlists: NavidromePlaylist[] | null;
    error: string | null;
  }
  let navPlaylists = $state.raw<ServerPlaylists[]>([]);
  let playlistsSeq = 0;

  async function refreshPlaylists(onlyServer?: string): Promise<void> {
    const targets = navProfiles.filter((p) => onlyServer == null || p.id === onlyServer);
    if (targets.length === 0) {
      if (onlyServer == null) navPlaylists = [];
      return;
    }
    const seq = ++playlistsSeq;
    if (onlyServer == null) {
      navPlaylists = targets.map((p) => ({
        serverId: p.id,
        name: p.name,
        playlists: null,
        error: null,
      }));
    }
    await Promise.all(
      targets.map(async (profile) => {
        try {
          const list = await navidromePlaylists(profile.id);
          if (seq !== playlistsSeq) return;
          const entry: ServerPlaylists = {
            serverId: profile.id,
            name: profile.name,
            playlists: list,
            error: null,
          };
          navPlaylists = navPlaylists.some((s) => s.serverId === profile.id)
            ? navPlaylists.map((s) => (s.serverId === profile.id ? entry : s))
            : [...navPlaylists, entry];
        } catch (e) {
          if (seq !== playlistsSeq) return;
          const message = friendlyIpcError("navidrome playlists", e);
          const entry: ServerPlaylists = {
            serverId: profile.id,
            name: profile.name,
            playlists: null,
            error: message,
          };
          navPlaylists = navPlaylists.some((s) => s.serverId === profile.id)
            ? navPlaylists.map((s) => (s.serverId === profile.id ? entry : s))
            : [...navPlaylists, entry];
        }
      }),
    );
  }

  $effect(() => {
    void openGroups.navidrome;
    void navProfiles;
    if (!openGroups.navidrome || navProfiles.length === 0) return;
    void refreshPlaylists();
  });

  const playlistTotal = $derived(
    navPlaylists.reduce((n, server) => n + (server.playlists?.length ?? 0), 0),
  );

  async function syncNow(serverId: string | null = null): Promise<void> {
    if (navSyncing) return;
    navSyncError = null;
    navSyncing = true;
    navProgress = null;
    try {
      await navidromeSync(serverId);
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

  /**
   * Settings changed (connect/switch/remove/prefs): refresh status + the
   * profile list, and pull the active server when it has no remote rows yet.
   */
  async function onNavChanged(): Promise<void> {
    await refreshNavStatus();
    await refreshNavProfiles();
    if (navStatus?.configured === true && navStatus.trackCount === 0 && !navSyncing) {
      onlog("navidrome: no remote tracks for the active server — starting a sync");
      void syncNow(navStatus.profileId);
    }
  }

  async function toggleStar(t: Track): Promise<void> {
    if (t.source !== REMOTE_SOURCE || t.remoteId == null) return;
    const id = t.remoteId;
    const key = starKey(t.serverId, id);
    const previous = isStarred(t);
    const starred = !previous;
    const next = new Map(starOverrides); // optimistic
    next.set(key, starred);
    starOverrides = next;
    try {
      await navidromeStar(t.serverId, id, starred);
      onlog(`navidrome: ${starred ? "★" : "☆"} ${t.title !== "" ? t.title : id}`);
    } catch (e) {
      const revert = new Map(starOverrides);
      if (previous === t.starred) revert.delete(key);
      else revert.set(key, previous);
      starOverrides = revert;
      onlog(`media library: star failed: ${e}`);
    }
  }

  // ── ratings (0–5 stars) ───────────────────────────────────────────────────
  // Track rows are raw (replaced wholesale on refresh, never mutated in
  // place), so an edit lands in this id-keyed override map first and the DB
  // follows. One composite button per row (not five buttons) keeps the
  // 100k-row DOM budget in check; the click x maps onto a star cell.

  let ratingOverrides = $state.raw<Map<number, number>>(new Map());

  function ratingOf(t: Track): number {
    return ratingOverrides.get(t.id) ?? t.rating;
  }

  async function rateTrack(t: Track, rating: number): Promise<void> {
    const previous = ratingOf(t);
    const next = new Map(ratingOverrides);
    next.set(t.id, rating);
    ratingOverrides = next;

    try {
      await librarySetRating(t.path, rating);
    } catch (e) {
      const revert = new Map(ratingOverrides);
      if (previous === t.rating) revert.delete(t.id);
      else revert.set(t.id, previous);
      ratingOverrides = revert;
      onlog(`media library: rating failed: ${e}`);
      return;
    }

    const label = t.title !== "" ? t.title : baseName(t.path);
    onlog(`media library: ${rating === 0 ? "cleared rating" : `${rating}★`} — ${label}`);

    // Remote rows also live on the server; a failed push keeps the local
    // rating (and logs it) — the next sync never overwrites a rated row.
    if (t.source === REMOTE_SOURCE && t.remoteId != null && navStatus?.configured === true) {
      try {
        await navidromeRate(t.serverId, t.remoteId, rating);
      } catch (e) {
        onlog(`navidrome: rating push failed (kept locally): ${e}`);
      }
    }
  }

  // ── row context menu (play / enqueue / star / rating) ───────────────────

  interface RowMenu {
    x: number;
    y: number;
    track: Track;
    /** Play/enqueue apply to the whole selection when >1 rows are selected. */
    scope: "row" | "selection";
    count: number;
  }
  let rowMenu = $state<RowMenu | null>(null);

  const rowMenuItems = $derived.by((): ContextMenuItem[] => {
    const m = rowMenu;
    if (!m) return [];
    const t = m.track;
    const bulk = m.scope === "selection";
    const list: ContextMenuItem[] = [
      {
        id: "play",
        label: bulk ? `Play ${m.count.toLocaleString()} selected` : "Play now",
      },
      {
        id: "enqueue",
        label: bulk ? `Enqueue ${m.count.toLocaleString()} selected` : "Enqueue",
      },
      { id: "copy", label: "Copy path" },
      { id: "sep", label: "", separator: true },
    ];
    if (t.source === REMOTE_SOURCE && t.remoteId != null) {
      list.push({
        id: "star",
        label: isStarred(t) ? "Unstar on Navidrome" : "Star on Navidrome",
      });
    }
    list.push({ id: "rate", label: "rating", rating: ratingOf(t) });

    // AUTO associations: capture the curve currently in the EQ engine for
    // this track/artist (the EQ window reapplies it on the next match).
    const trackEq = getTrackEq(t.path) != null;
    const artistEq = t.artist.trim() !== "" && getArtistEq(t.artist) != null;
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

  function openRowMenu(e: MouseEvent, t: Track): void {
    e.preventDefault();
    const inSelection = selectAllMode || selectedIds.has(t.id);
    if (!inSelection) {
      // Right-clicking outside the selection makes that row current, so the
      // menu's per-row actions and the visible selection agree.
      selectAllMode = false;
      selectedIds = new Set([t.id]);
      const index = displayed.findIndex((row) => row.id === t.id);
      anchorIndex = index >= 0 ? index : null;
    }
    const count = inSelection
      ? selectAllMode
        ? displayedTotal
        : selectedMetas.length
      : 1;
    rowMenu = {
      x: e.clientX,
      y: e.clientY,
      track: t,
      scope: inSelection && count > 1 ? "selection" : "row",
      count: Math.max(1, count),
    };
  }

  /** Rows behind a bulk context-menu Play/Enqueue (virtual ALL included). */
  function menuSelectionRows(): Promise<Track[]> {
    return selectAllMode ? allRowsForAction() : Promise.resolve(selectedMetas);
  }

  function rowMenuSelect(id: string): void {
    const m = rowMenu;
    rowMenu = null;
    if (!m) return;
    const t = m.track;
    if (id === "play") {
      if (m.scope === "selection") {
        void menuSelectionRows().then((rows) => {
          if (rows.length > 0) onplaynow(toMetas(rows));
        });
      } else {
        onplaynow([toMeta(t)]);
      }
    } else if (id === "enqueue") {
      if (m.scope === "selection") {
        void menuSelectionRows().then((rows) => {
          if (rows.length > 0) onenqueue(toMetas(rows));
        });
      } else {
        onenqueue([toMeta(t)]);
      }
    } else if (id === "copy") {
      void navigator.clipboard.writeText(t.path).catch((e) => {
        onlog(`copy path failed: ${e}`);
      });
    } else if (id === "star") {
      void toggleStar(t);
    } else if (id.startsWith("rate:")) {
      const rating = Number(id.slice("rate:".length));
      if (Number.isFinite(rating)) void rateTrack(t, rating);
    } else if (id === "eq-track") {
      void rememberEq(t.path, t.artist, "track", t.title)
        .then(() => onlog(`eq: saved the current curve for “${t.title || t.path}”`))
        .catch((e) => onlog(`eq: save failed: ${e}`));
    } else if (id === "eq-artist") {
      void rememberEq(t.path, t.artist, "artist")
        .then(() => onlog(`eq: saved the current curve for ${t.artist || "artist"}`))
        .catch((e) => onlog(`eq: save failed: ${e}`));
    } else if (id === "eq-clear") {
      void clearEqFor(t.path, t.artist)
        .then((changed) => {
          if (changed) onlog(`eq: cleared the track/artist curve for “${t.title || t.path}”`);
        })
        .catch((e) => onlog(`eq: clear failed: ${e}`));
    }
  }

  let lastProgressRefresh = 0;
  let lastStatsRefresh = 0;
  let lastTreeRefresh = 0;
  $effect(() => {
    void refreshNavStatus();
    void refreshNavProfiles();
    const up = onNavidromeProgress((p) => {
      // Events also fire for syncs started elsewhere — latch onto them.
      navSyncing = true;
      navProgress = p;
      if (document.hidden) return; // no point repainting a hidden window
      const now = performance.now();

      // Header counts + the backend's remote count are cheap COUNT queries.
      if (now - lastStatsRefresh >= 2000) {
        lastStatsRefresh = now;
        refreshStats();
        void refreshNavStatus();
      }

      // Filtered panes (artist/album/…) requery at most every 5s and never
      // while the previous query is in flight. ALL/REMOTE are paged: their
      // counts follow stats; the page refreshes on view entry/completion.
      const masterView =
        selection.kind === "all" ||
        selection.kind === "remote" ||
        selection.kind === "remoteServer" ||
        selection.kind === "server";
      if (!masterView && !nodeLoading && now - lastProgressRefresh >= 5000) {
        lastProgressRefresh = now;
        dataVersion++;
      }

      // Only the currently expanded tree groups refresh, on a slow cadence.
      if (now - lastTreeRefresh >= 15_000) {
        lastTreeRefresh = now;
        refreshTreeLists();
      }
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
        void refreshNavProfiles();
        void refreshAll();
        ratingOverrides = new Map(); // pick up server-seeded ratings
        starOverrides = new Map(); // ...and server-owned stars
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

  /** navidrome coverArt id → authed URL (session-local, keyed by server+id). */
  let remoteArtUrls = $state.raw<Map<string, string>>(new Map());
  /** Art keys whose <img> errored (dead link) → back to the placeholder. */
  let failedArtIds = $state.raw<Set<string>>(new Set());
  /** Keys already queued for `fetchRemoteArt`; each is fetched at most once. */
  const requestedRemoteArt = new Set<string>();

  const REMOTE_ART_BATCH = 8;

  function artKey(serverId: string | null | undefined, id: string): string {
    return `${serverId ?? ""}/${id}`;
  }

  function albumCoverSrc(a: Album): string | null {
    const id = a.artId;
    if (id == null) return null;
    if (a.artSource === REMOTE_SOURCE) {
      const key = artKey(a.artServerId, id);
      if (failedArtIds.has(key)) return null;
      return remoteArtUrls.get(key) ?? null;
    }
    if (a.artSource === "local") return localArtUrl(id);
    return null;
  }

  function failArt(serverId: string | null, id: string): void {
    const key = artKey(serverId, id);
    if (failedArtIds.has(key)) return;
    const next = new Set(failedArtIds);
    next.add(key);
    failedArtIds = next;
  }

  interface RemoteArtRef {
    serverId: string | null;
    id: string;
  }

  async function fetchRemoteArt(refs: RemoteArtRef[]): Promise<void> {
    for (let i = 0; i < refs.length; i += REMOTE_ART_BATCH) {
      const batch = refs.slice(i, i + REMOTE_ART_BATCH);
      const results = await Promise.allSettled(
        batch.map((ref) => navidromeArtUrl(ref.serverId, ref.id, 64)),
      );
      const next = new Map(remoteArtUrls);
      let changed = false;
      results.forEach((r, k) => {
        if (r.status === "fulfilled") {
          next.set(artKey(batch[k].serverId, batch[k].id), r.value);
          changed = true;
        }
      });
      if (changed) remoteArtUrls = next;
    }
  }

  $effect(() => {
    if (!openGroups.albums) return;
    const wanted: RemoteArtRef[] = [];
    for (const a of albumsWindowed) {
      const id = a.artId;
      if (id == null || a.artSource !== REMOTE_SOURCE) continue;
      const key = artKey(a.artServerId, id);
      if (
        !remoteArtUrls.has(key) &&
        !failedArtIds.has(key) &&
        !requestedRemoteArt.has(key)
      ) {
        requestedRemoteArt.add(key);
        wanted.push({ serverId: a.artServerId, id });
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
      // resolves the stream URL through serverId + remoteId (queue.feedPath).
      ...(t.source === REMOTE_SOURCE && t.remoteId != null
        ? { remoteId: t.remoteId, ...(t.serverId != null ? { serverId: t.serverId } : {}) }
        : {}),
    };
  }

  function toMetas(list: Track[]): QueueItemMeta[] {
    return list.map(toMeta);
  }

  let actionBusy = $state(false);

  /** Load every row of the current paged view for an ENQUEUE/PLAY ALL. */
  async function allRowsForAction(): Promise<Track[]> {
    if (!pagedView) return displayed;
    const total = displayedTotal;
    if (pagedRows.length >= total) return pagedRows;
    actionBusy = true;
    try {
      const rows: Track[] = [];
      let offset = 0;
      const CHUNK = 2000;
      while (offset < total) {
        const page = await libraryTracks({
          source:
            selection.kind === "remote" || selection.kind === "remoteServer"
              ? REMOTE_SOURCE
              : undefined,
          serverId: selection.kind === "remoteServer" ? selection.value : undefined,
          limit: CHUNK,
          offset,
        });
        rows.push(...page);
        offset += page.length;
        if (page.length === 0) break;
      }
      return rows;
    } catch (e) {
      onlog(`media library: bulk load failed: ${e}`);
      return pagedRows;
    } finally {
      actionBusy = false;
    }
  }

  async function enqueueAll(): Promise<void> {
    const rows = await allRowsForAction();
    if (rows.length > 0) onenqueue(toMetas(rows));
  }

  async function enqueueSelected(): Promise<void> {
    // SELECT ALL covers rows that were never loaded into the table; fetch
    // the full filtered set only when the action actually needs it.
    const rows = selectAllMode ? await allRowsForAction() : selectedMetas;
    if (rows.length > 0) onenqueue(toMetas(rows));
  }

  async function playAll(): Promise<void> {
    const rows = await allRowsForAction();
    if (rows.length > 0) onplaynow(toMetas(rows));
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
      <button onclick={() => void enqueueAll()} disabled={displayedTotal === 0 || actionBusy} title="append every track in view">ENQUEUE ALL</button>
      <button
        onclick={enqueueSelected}
        disabled={selectedCount === 0 || actionBusy}
        title="append the selected tracks">ENQUEUE SEL</button>
      <button onclick={() => void playAll()} disabled={displayedTotal === 0 || actionBusy} title="append and start playing">PLAY ALL</button>
    </div>
    <span class="statline" title={roots.join("\n")}>{statsLine}</span>
  </div>
  {#if searchError}
    <div class="search-err">{searchError}</div>
  {/if}

  <div class="body">
    <nav class="tree" aria-label="library categories" bind:this={treeEl} onscroll={onTreeScroll}>
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
        <span class="cnt">({(chipStats?.artists ?? 0).toLocaleString()})</span>
      </button>
      {#if openGroups.artists}
        {#if artistNames.length === 0 && treeError == null}
          <div class="leaf empty">{navSyncing ? "syncing…" : "no artists"}</div>
        {:else if artistNames.length > 0}
          <div
            class="vlist"
            bind:this={artistsListEl}
            style="height:{listHeight(artistNames.length, TREE_LEAF_H)}px">
            {#if artistNames.length >= TREE_JUMP_MIN}
              <div class="jump" role="group" aria-label="jump to artist initial">
                {#each artistInitials as letter (letter)}
                  <button
                    class="jbtn"
                    class:on={artistJumpLetter === letter}
                    title={`jump to ${letter}`}
                    onclick={() => jumpTo("artists", letter)}>{letter}</button>
                {/each}
              </div>
            {/if}
            <div
              class="vstrip"
              style="top:{railHeight(artistNames.length) + artistWindow.start * TREE_LEAF_H}px">
              {#each artistsWindowed as name (name)}
                <button
                  class="leaf"
                  class:on={selKey(selection) === `artist:${name}`}
                  onclick={() => select({ kind: "artist", value: name })}>{name}</button>
              {/each}
            </div>
          </div>
        {/if}
      {/if}

      <button
        class="node"
        onclick={() => (openGroups.albums = !openGroups.albums)}
        aria-expanded={openGroups.albums}>
        <span class="tw">{openGroups.albums ? "▾" : "▸"}</span>ALBUMS
        <span class="cnt">({(chipStats?.albums ?? 0).toLocaleString()})</span>
      </button>
      {#if openGroups.albums}
        {#if albums.length === 0 && treeError == null}
          <div class="leaf empty">{navSyncing ? "syncing…" : "no albums"}</div>
        {:else if albums.length > 0}
          <div
            class="vlist"
            bind:this={albumsListEl}
            style="height:{listHeight(albums.length, TREE_ALBUM_H)}px">
            {#if albums.length >= TREE_JUMP_MIN}
              <div class="jump" role="group" aria-label="jump to album artist initial">
                {#each albumInitials as letter (letter)}
                  <button
                    class="jbtn"
                    class:on={albumJumpLetter === letter}
                    title={`jump to ${letter}`}
                    onclick={() => jumpTo("albums", letter)}>{letter}</button>
                {/each}
              </div>
            {/if}
            <div
              class="vstrip"
              style="top:{railHeight(albums.length) + albumWindow.start * TREE_ALBUM_H}px">
              {#each albumsWindowed as a (a.albumKey)}
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
                      onerror={() => a.artId != null && failArt(a.artServerId, a.artId)} />
                  {:else}
                    <span class="alb-art none" aria-hidden="true">♪</span>
                  {/if}
                  <span class="alb-name">{a.name}{a.year != null ? ` (${a.year})` : ""}</span>
                </button>
              {/each}
            </div>
          </div>
        {/if}
      {/if}

      <button
        class="node"
        onclick={() => (openGroups.genres = !openGroups.genres)}
        aria-expanded={openGroups.genres}>
        <span class="tw">{openGroups.genres ? "▾" : "▸"}</span>GENRES
        <span class="cnt">({(chipStats?.genres ?? 0).toLocaleString()})</span>
      </button>
      {#if openGroups.genres}
        {#if genreNames.length === 0 && treeError == null}
          <div class="leaf empty">{navSyncing ? "syncing…" : "no genres"}</div>
        {:else if genreNames.length > 0}
          <div
            class="vlist"
            bind:this={genresListEl}
            style="height:{listHeight(genreNames.length, TREE_LEAF_H)}px">
            {#if genreNames.length >= TREE_JUMP_MIN}
              <div class="jump" role="group" aria-label="jump to genre initial">
                {#each genreInitials as letter (letter)}
                  <button
                    class="jbtn"
                    class:on={genreJumpLetter === letter}
                    title={`jump to ${letter}`}
                    onclick={() => jumpTo("genres", letter)}>{letter}</button>
                {/each}
              </div>
            {/if}
            <div
              class="vstrip"
              style="top:{railHeight(genreNames.length) + genreWindow.start * TREE_LEAF_H}px">
              {#each genresWindowed as name (name)}
                <button
                  class="leaf"
                  class:on={selKey(selection) === `genre:${name}`}
                  onclick={() => select({ kind: "genre", value: name })}>{name}</button>
              {/each}
            </div>
          </div>
        {/if}
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
        {#if navProfiles.length > 0}
          <div class="leaf sub">
            <span class="sublabel">SERVERS</span>
            <span class="cnt">({navProfiles.length})</span>
          </div>
          {#each navProfiles as profile (profile.id)}
            {@const serverStats = stats?.navidromeServers?.find(
              (s) => s.serverId === profile.id,
            )}
            <div class="leaf smart" class:on={selKey(selection) === `remoteServer:${profile.id}`}>
              <button
                class="leafbtn"
                title={`${profile.url} — ${(serverStats?.tracks ?? 0).toLocaleString()} synced tracks`}
                onclick={() => select({ kind: "remoteServer", value: profile.id })}>
                {profile.name}{#if profile.active} ●{/if}
              </button>
              <span class="cnt">({(serverStats?.tracks ?? 0).toLocaleString()})</span>
            </div>
          {/each}
        {/if}
        {#if navProfiles.length > 0}
          <div class="leaf sub">
            <span class="sublabel">PLAYLISTS</span>
            <span class="cnt">({playlistTotal})</span>
            <button
              class="edit"
              title="reload playlists from every server"
              onclick={() => void refreshPlaylists()}>↻</button>
          </div>
          {#each navPlaylists as server (server.serverId)}
            <div class="leaf sub">
              <span class="sublabel">{server.name}</span>
              <span class="cnt">({server.playlists?.length ?? 0})</span>
            </div>
            {#if server.error}
              <div class="leaf empty err" title={server.error}>playlists unavailable</div>
            {:else if server.playlists == null}
              <div class="leaf empty">loading playlists…</div>
            {:else if server.playlists.length === 0}
              <div class="leaf empty">no playlists</div>
            {:else}
              {#each server.playlists as p (p.id)}
                <div
                  class="leaf smart"
                  class:on={selKey(selection) === `playlist:${server.serverId}:${p.id}`}>
                  <button
                    class="leafbtn"
                    title={`${p.songCount} track${p.songCount === 1 ? "" : "s"} on ${server.name}`}
                    onclick={() => {
                      selectedIds = new Set(); // playlist rows use ephemeral ids
                      selectAllMode = false;
                      select({ kind: "playlist", value: p.id, server: server.serverId });
                    }}>{p.name}</button>
                  <span class="cnt">{p.songCount}</span>
                </div>
              {/each}
            {/if}
          {/each}
        {/if}
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
            <div class="kv"><span>profile</span><b>{navStatus.profileLabel ?? navStatus.profileId ?? "—"}</b></div>
            <div class="kv"><span>url</span><b>{navStatus.url}</b></div>
            <div class="kv"><span>user</span><b>{navStatus.user}</b></div>
            <div class="kv"><span>version</span><b>{navStatus.serverVersion ?? "—"}</b></div>
            <div class="kv"><span>library</span><b>{navStatus.libraryId ?? "all libraries"}</b></div>
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
              onclick={() => void syncNow()}
              disabled={navSyncing || navStatus?.configured !== true}
              title="sync every saved server">{navSyncing ? "SYNCING…" : "SYNC NOW"}</button>
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
          ><span class="c-time">TIME</span
          ><span class="c-rating" title="rating (click a star; the same star clears)">RATING</span
          ><span class="c-star" title="star (navidrome)">★</span>
        </div>
        <div
          class="rows"
          role="listbox"
          aria-label="track list"
          bind:this={rowsEl}
          onscroll={onRowsScroll}>
          {#if viewError}
            <div class="msg err">{viewError}</div>
          {:else if tableLoading}
            <div class="msg">loading…</div>
          {:else if displayed.length === 0}
            {#if (selection.kind === "remote" || selection.kind === "remoteServer") && searchResults == null}
              <div class="msg remote-empty">
                {#if navSyncing}
                  <p>
                    syncing the Navidrome library…
                    {navProgress
                      ? `${navProgress.label ? `${navProgress.label} · ` : ""}${navProgress.scanned.toLocaleString()} / ${navProgress.total.toLocaleString()}`
                      : ""}
                  </p>
                {:else}
                  <p>
                    {navSyncError
                      ? `last sync failed: ${navSyncError}`
                      : navStatus?.configured === true
                        ? "the server library is not synced into this app yet"
                        : "no server connection — open SERVER STATUS to connect"}
                  </p>
                  <button
                    class="cta"
                    disabled={navStatus?.configured !== true || navSyncing}
                    onclick={() =>
                      void syncNow(selection.kind === "remoteServer" ? selection.value : null)}>
                    {navSyncError ? "RETRY SYNC" : "SYNC LIBRARY NOW"}
                  </button>
                {/if}
              </div>
            {:else}
              <div class="msg">{emptyMessage}</div>
            {/if}
          {:else}
            <div style="height:{windowStart * ROW_H}px" aria-hidden="true"></div>
            {#each windowRows as t, i (t.id)}
              <div
                class="trow"
                class:selected={isRowSelected(t)}
                role="option"
                aria-selected={isRowSelected(t)}
                tabindex="-1"
                title={t.path}
                onpointerdown={(e) => rowDown(e, windowStart + i)}
                ondblclick={() => onplaynow([toMeta(t)])}
                oncontextmenu={(e) => openRowMenu(e, t)}>
                <span class="c-num">{windowStart + i + 1}</span>
                <span class="c-title">{#if t.source === REMOTE_SOURCE}<span class="rmark" title="navidrome track">◈ </span>{/if}{t.title !== "" ? t.title : baseName(t.path)}</span>
                <span class="c-artist">{t.artist}</span>
                <span class="c-album">{t.album}</span>
                <span class="c-time">{fmt(t.durationSecs)}</span>
                <span class="c-rating">
                  <button
                    class="rstars"
                    aria-label="rating {ratingOf(t)} of 5 stars"
                    title="rating {ratingOf(t)}/5 — click a star to set, click the same star to clear"
                    onpointerdown={(e) => e.stopPropagation()}
                    ondblclick={(e) => e.stopPropagation()}
                    onclick={(e) => {
                      if (e.detail > 1) return; // ignore the 2nd click of a dblclick
                      // Pointer: the x position picks a star cell (clicking
                      // the current rating clears it). Keyboard activation
                      // has no x → cycle 1..5→0.
                      let rating: number;
                      if (e.detail === 0) {
                        rating = ratingOf(t) >= 5 ? 0 : ratingOf(t) + 1;
                      } else {
                        const r = e.currentTarget.getBoundingClientRect();
                        const star = Math.min(
                          5,
                          Math.max(1, Math.ceil(((e.clientX - r.left) / r.width) * 5)),
                        );
                        rating = ratingOf(t) === star ? 0 : star;
                      }
                      void rateTrack(t, rating);
                    }}>
                    <span class="ron">{"★".repeat(ratingOf(t))}</span
                    ><span class="roff">{"☆".repeat(5 - ratingOf(t))}</span>
                  </button>
                </span>
                <span class="c-star">
                  {#if t.source === REMOTE_SOURCE && t.remoteId != null}
                    <button
                      class="star"
                      class:on={isStarred(t)}
                      title={isStarred(t) ? "unstar on server" : "star on server"}
                      onpointerdown={(e) => e.stopPropagation()}
                      ondblclick={(e) => e.stopPropagation()}
                      onclick={() => void toggleStar(t)}>
                      {isStarred(t) ? "★" : "☆"}
                    </button>
                  {/if}
                </span>
              </div>
            {/each}
            <div
              style="height:{Math.max(0, (displayed.length - windowEnd) * ROW_H)}px"
              aria-hidden="true"></div>
          {/if}
        </div>
      </section>
    {/if}
  </div>

  <footer class="bar">
    {#if navSyncing}
      <span class="syncnote">
        navidrome syncing{navProgress?.label ? ` ${navProgress.label}` : ""}…
        {navProgress
          ? `${navProgress.scanned.toLocaleString()} / ${navProgress.total.toLocaleString()}`
          : ""}
      </span>
    {/if}
    <span class="totals">{footerText}</span>
    {#if selectedCount > 0}
      <span class="selnote">{selectedCount.toLocaleString()} selected</span>
    {/if}
    <button class="selall" onclick={selectAllView} disabled={displayedTotal === 0}
      title={selectAllMode
        ? "clear the selection"
        : `select all ${displayedTotal.toLocaleString()} tracks in this view`}>
      {selectAllMode ? "DESELECT ALL" : "SELECT ALL"}
    </button>
  </footer>

  {#if editing}
    <SmartViewEditor
      id={editing.id}
      initial={editing.def}
      onsave={(id) => void onEditorSave(id)}
      ondelete={(id) => void onEditorDelete(id)}
      onpublish={onEditorPublish}
      publishServers={navProfiles.map((p) => ({ id: p.id, name: p.name }))}
      defaultPublishServer={navStatus?.profileId ?? null}
      oncancel={() => (editing = null)} />
  {/if}

  {#if showSettings}
    <NavidromeSettings
      onclose={() => (showSettings = false)}
      onchanged={() => void onNavChanged()}
      onlog={onlog} />
  {/if}

  {#if rowMenu}
    <ContextMenu
      x={rowMenu.x}
      y={rowMenu.y}
      items={rowMenuItems}
      onselect={rowMenuSelect}
      onclose={() => (rowMenu = null)} />
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
    overflow: hidden;
  }

  /* ── left tree ── */
  .tree {
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
    background: #0d0d0d;
    border-right: 1px solid #2a2a2a;
    padding: 4px;
    display: flex;
    flex-direction: column;
    gap: 1px;
    position: relative; /* offsetParent for the virtual list windows */
    scrollbar-width: thin;
    scrollbar-color: #2f4f33 #000;
  }

  /* Never shrink tree rows to fit the fixed-height panel: with hundreds of
     leaves a default flex-shrink compresses them to illegible slivers. The
     tree scrolls instead. */
  .tree > * {
    flex: none;
  }

  /* Virtual leaf lists (artists / albums / genres): the container keeps the
     full scroll height, the strip inside holds only the windowed rows.
     `overflow: clip` (not hidden) keeps the sticky A–Z rail anchored to the
     tree scrollport: hidden would make this non-scrolling box a scroll
     container of its own and freeze the rail. */
  .tree .vlist {
    position: relative;
    overflow: clip;
  }

  /* Compact two-row A–Z rail: 14 monospace letter cells per row fills the
     176px tree column without horizontal scrolling. */
  .tree .jump {
    position: sticky;
    top: 0;
    z-index: 3;
    box-sizing: border-box;
    height: 24px;
    padding: 1px 0;
    display: grid;
    grid-template-columns: repeat(14, minmax(0, 1fr));
    grid-auto-rows: 11px;
    background: #000;
    border-bottom: 1px solid #1e3a24;
  }

  .tree .jump .jbtn {
    min-width: 0;
    height: 11px;
    padding: 0;
    border: 0;
    border-radius: 0;
    background: transparent;
    color: #3f7a49;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 8px;
    line-height: 11px;
    text-align: center;
    cursor: pointer;
  }

  .tree .jump .jbtn:hover {
    color: #8dff9e;
    background: #12301b;
  }

  .tree .jump .jbtn.on {
    color: #0d0d0d;
    background: #3ccb4e;
  }

  .tree .vstrip {
    position: absolute;
    left: 0;
    right: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  /* Fixed 19px row + the strip's 1px gap = TREE_LEAF_H (20). The album rule
     below overrides height/line-height (28 + 1 = TREE_ALBUM_H). */
  .tree .vstrip .leaf {
    box-sizing: border-box;
    height: 19px;
    line-height: 19px;
    padding-top: 0;
    padding-bottom: 0;
    border-width: 0;
    content-visibility: visible;
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

  /* Static sub-heading inside a tree group (e.g. NAVIDROME → PLAYLISTS). */
  .leaf.sub {
    display: flex;
    align-items: center;
    gap: 4px;
    padding-left: 20px;
    color: #6f6f6f;
    font-size: 10px;
    letter-spacing: 0.08em;
    content-visibility: visible;
  }

  .leaf.sub .sublabel {
    flex: 1;
    min-width: 0;
  }

  .leaf.sub .edit {
    flex: none;
    border-color: transparent;
    background: transparent;
    color: #4f4f4f;
    padding: 1px 4px;
  }

  .leaf.sub .edit:hover {
    color: #3ccb4e;
  }

  .leaf.smart .cnt {
    flex: none;
    padding-right: 4px;
    color: #2f6f3a;
    font-size: 10px;
  }

  /* ── right table ── */
  .table {
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    display: flex;
    flex-direction: column;
    background: #000;
  }

  .thead,
  .trow {
    display: grid;
    grid-template-columns: 4ch minmax(8ch, 1.6fr) minmax(6ch, 1fr) minmax(6ch, 1fr) 6ch 58px 14px;
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
    overscroll-behavior: contain;
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
    /* The table is virtualized (only ~visible rows are in the DOM); no
       content-visibility needed here. */
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

  .c-rating {
    text-align: center;
  }

  .thead .c-star,
  .thead .c-rating {
    color: #4f4f4f;
    font-size: 10px;
  }

  /* One composite button per row: click x picks the star cell (1..5).
     Five glyphs inside keeps the DOM budget for 100k-row lists. */
  .trow .rstars {
    background: none;
    border: none;
    padding: 0;
    font-size: 10px;
    line-height: 1;
    letter-spacing: 0;
    white-space: nowrap;
    cursor: pointer;
  }

  .trow .rstars .ron {
    color: #d8a63c;
  }

  .trow .rstars .roff {
    color: #3a5a3f;
  }

  .trow .rstars:hover .ron {
    color: #ffd76a;
  }

  .trow .rstars:hover .roff {
    color: #3ccb4e;
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

  /* REMOTE TRACKS empty state: the pane owns the sync call-to-action. */
  .remote-empty {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 8px;
    padding: 10px;
  }

  .remote-empty p {
    margin: 0;
    color: #4a7a52;
  }

  .remote-empty .cta {
    background: #12301b;
    color: #8dff9e;
    border: 1px solid #2f4f33;
    border-radius: 2px;
    padding: 3px 10px;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 11px;
    letter-spacing: 0.08em;
    cursor: pointer;
  }

  .remote-empty .cta:hover:not(:disabled) {
    border-color: #3ccb4e;
    color: #3ccb4e;
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

  .syncnote {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 10px;
    color: #d8a63c;
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
