/**
 * Play queue + engine correlation (M3).
 *
 * ── Engine correlation protocol ("expectations") ────────────────────────────
 * The audio engine owns its playback queue and only reports progress through
 * `audio-event`s; its track ids are engine-internal u64s. The UI therefore
 * correlates by FEED ORDER:
 *
 *  · `expectations` lists every track we fed to the engine (via audio_play /
 *    audio_set_upcoming) that has not started yet, oldest first — the front
 *    entry is the queue item the next TrackStarted is expected to belong to.
 *  · TrackStarted shifts the front entry; the shifted item becomes
 *    currentIndex. This is the ONLY event that moves the position — it is
 *    how a gapless engine-side advance updates the UI.
 *  · TrackFinished never moves the position: audio_play REPLACES the engine
 *    queue and emits TrackFinished for the replaced track, so "finished"
 *    does not imply "advance on our side".
 *  · audio_set_upcoming declares the ordered window of the next
 *    PREFETCH_AHEAD tracks. The engine pre-opens the first few (bounded), so
 *    an automatic advance is gapless and a skip forward does not wait for a
 *    network open. Updates are reconciled by item id: re-declaring the same
 *    tail after every advance is a no-op, and a reorder/removal keeps
 *    entries that are already open. reconcileAhead recomputes the window
 *    after every mutation, toggle and TrackStarted.
 *
 * Edge cases:
 *  · Play-replace during gapless — play(i) resets the engine queue and our
 *    expectations to [i, ...window]. A straggler TrackStarted from the
 *    replaced pipeline then just consumes the front entry; since exactly one
 *    entry is consumed per TrackStarted and expectations always mirrors
 *    fed-but-unstarted tracks, the position converges (worst case one
 *    transient mis-highlight).
 *  · Enqueue at queue end — no successor exists, nothing is fed; the engine
 *    runs dry and emits EndOfQueue: repeat=all wraps via play(0), anything
 *    else is a stop state (currentIndex stays, engineLive=false).
 *  · repeat=one feeds the current track as its own successor, making repeats
 *    gapless; the EndOfQueue wrap is then only a fallback.
 *  · The window walk stops when it revisits a position, so a short queue
 *    under repeat=all cannot enqueue the same items forever.
 *
 * Remote (M5): items synced from Navidrome carry
 * `path: "navidrome://<serverId>/<remoteId>"`
 * plus `remoteId`. Inside the serialized feed chain `feedPath` translates
 * them into a fresh stream URL (they can be time-limited, so every feed
 * resolves its own); a failed resolution rejects into the existing
 * feed-error log and the item simply never plays (like a bad file). The
 * same TrackStarted/EndOfQueue/stop transitions drive Subsonic scrobbling
 * and, symmetrically, local play stats (library_mark_played) — see the
 * scrobble section below the feed helpers.
 *
 * Entries carry the item id next to the fed index so list edits (remove /
 * move / insert) repair indexes by identity — a bare index would go stale
 * after a splice. If a fed item is removed from the list the engine still
 * plays its file out once (audio_set_upcoming can cancel entries that are
 * still opening, but not one that already started); the UI simply does not
 * move for it.
 */

import { invoke } from "@tauri-apps/api/core";

import { IS_PLAYER } from "../window/identity";

import { friendlyIpcError, libraryMarkPlayed } from "../library/ipc";
import { navidromeScrobble, navidromeStreamUrl } from "../navidrome/ipc";
import { streamsResolve } from "../streams/ipc";

export type RepeatMode = "off" | "all" | "one";

export interface QueueItem {
  /** Stable UI identity (survives reorders; the engine has its own u64 ids). */
  id: string;
  /**
   * Local file path, an `http(s)://` playlist URL, or
   * `navidrome://<serverId>/<remoteId>` for synced remote rows. Remote items
   * resolve their real stream URL at feed time (feedPath).
   */
  path: string;
  title: string;
  artist: string;
  durationSecs: number | null;
  /**
   * Set when `path` is a link that needs a per-play resolver rather than a
   * direct media URL. Currently only YouTube (resolved through yt-dlp).
   */
  streamKind?: "youtube";
  /**
   * Navidrome track id — present only on remote items; drives stream-URL
   * resolution and scrobbling. `null`/undefined ⇒ a local file.
   */
  remoteId?: string;
  /** Navidrome profile id paired with `remoteId` (aggregated servers). */
  serverId?: string;
  /**
   * Server-relative file path when the enqueuing library row had one —
   * carried so UI actions (Copy path) never depend on an async library
   * lookup. Undefined for local files and remote rows synced before the
   * column existed.
   */
  remotePath?: string;
}

/** Fully-tagged track info; the M4 library browser will feed this directly. */
export type QueueItemMeta = Omit<QueueItem, "id">;

export type EngineEvent =
  | { type: "TrackStarted"; id: number; durationSecs: number | null }
  | { type: "TrackFinished"; id: number }
  | { type: "EndOfQueue" }
  | { type: "Error"; id: number | null; msg: string };

/** One fed-but-not-yet-started track (see the protocol notes above). */
interface Expectation {
  /** Queue index at feed time; superseded by the id lookup on mutations. */
  index: number;
  id: string;
}

/** Upcoming tracks declared to the engine (immediate next + 3 more). */
const PREFETCH_AHEAD = 4;

// Extensions the plan's decoder surface claims (§4): MP3, FLAC, OGG
// Vorbis/Opus, AAC/ALAC (M4A), WAV — plus the usual container spellings.
const AUDIO_EXTENSIONS = new Set([
  "mp3",
  "flac",
  "ogg",
  "oga",
  "opus",
  "m4a",
  "aac",
  "wav",
  "aif",
  "aiff",
]);

let items = $state<QueueItem[]>([]);
let currentIndex = $state<number | null>(null);
let shuffle = $state(false);
let repeat = $state<RepeatMode>("off");

// Engine-correlation bookkeeping. Deliberately non-reactive: nothing renders
// off it, and handleEngineEvent drives it synchronously.
let expectations: Expectation[] = [];
/** audio_play issued; its TrackStarted has not arrived yet. */
let startPending = false;
/** We believe the engine is playing/paused something we fed it. */
let engineLive = false;
/** List item the engine is audibly playing (null = never fed / orphaned). */
let playingItemId: string | null = null;
/** Permutation used for shuffle navigation; indexes into `items`. */
let shuffleOrder: number[] = [];
/** Serializes play→enqueue feeds so a fast double-click cannot interleave. */
let feedChain: Promise<void> = Promise.resolve();
/**
 * Engine source for each item in the current prefetch window (item id →
 * path/URL). Re-declaring a surviving tail reuses it, so YouTube links are
 * resolved by yt-dlp once per window entry, not once per advance.
 */
const windowSources = new Map<string, string>();
/** Distinguishes our feed from a newer play() when feeds resolve. */
let playSession = 0;
let idCounter = 0;
let log: ((line: string) => void) | null = null;

export function isAudioPath(path: string): boolean {
  const dot = path.lastIndexOf(".");
  const ext = dot >= 0 ? path.slice(dot + 1).toLowerCase() : "";
  return AUDIO_EXTENSIONS.has(ext);
}

/** True for a direct `http(s)://` stream URL (playlist URL rows). */
export function isStreamUrl(path: string): boolean {
  return /^https?:\/\//i.test(path);
}

export function formatTime(secs: number): string {
  const m = Math.floor(secs / 60);
  const s = Math.floor(secs % 60);
  return `${m}:${String(s).padStart(2, "0")}`;
}

// crypto.randomUUID is absent in non-secure webview contexts (some Tauri
// origins on WKWebView); fall back to a counter id so enqueue never throws.
function newId(): string {
  if (typeof crypto.randomUUID === "function") return crypto.randomUUID();
  return `q-${Date.now().toString(36)}-${(idCounter++).toString(36)}`;
}

function logLine(line: string): void {
  log?.(line);
}

/** Serialize feeds to the engine; IPC arrival order is not otherwise bound. */
function feed(run: () => Promise<unknown>): void {
  feedChain = feedChain
    .then(run)
    .catch((e) => logLine(`✖ feed error: ${e}`))
    .then(() => undefined);
}

/**
 * The path handed to the engine for `item`. Remote (Navidrome) items store
 * `navidrome://<serverId>/<remoteId>` and must be translated into a stream
 * URL first — URLs can be time-limited, so each feed resolves its own
 * instead of caching. A failed resolution rejects inside the serialized feed
 * chain and lands in the existing feed-error log above; the item is skipped
 * exactly like a file the engine cannot open.
 */
async function feedPath(item: QueueItem): Promise<string> {
  if (item.remoteId != null) {
    return navidromeStreamUrl(item.serverId, item.remoteId);
  }
  if (item.streamKind === "youtube") {
    // Direct googlevideo URLs expire, so resolve the canonical link again
    // every time this item is handed to the engine.
    return (await streamsResolve(item.path)).streamUrl;
  }
  return item.path;
}

// ── Navidrome scrobbling (M5) ──────────────────────────────────────────────
// Subsonic scrobbles come in two flavors: submission=false ("now playing")
// when a track starts, and submission=true when the listen counts. We keep
// at most ONE open instance (`scrobbleOpen`); a close submits exactly once
// and clears it, so a double-submit is impossible by construction:
//  · TrackStarted on a remote item closes whatever is open first — a same-id
//    restart (repeat-one wrap, or replaying the current track) closes its
//    own previous instance, which is the "new play" case — then opens a new
//    now-playing instance.
//  · TrackStarted on a local item, on an orphaned (removed) feed, EndOfQueue
//    and an explicit stop (onStopped) close the open instance.
//  · Seeks emit no engine events, so they can never resubmit; pause and list
//    edits keep the instance open because the audio keeps playing.
//  · clear() deliberately leaves the instance open: the engine keeps playing
//    what it holds and EndOfQueue (or the next TrackStarted) closes it.

/** The open now-playing instance, if the current track is a remote one. */
let scrobbleOpen: { itemId: string; remoteId: string; serverId?: string } | null = null;

function scrobbleNow(
  serverId: string | undefined,
  remoteId: string,
  submission: boolean,
): void {
  navidromeScrobble(serverId, remoteId, submission).catch((e) =>
    logLine(`✖ scrobble failed (${submission ? "submit" : "now-playing"} ${remoteId}): ${e}`),
  );
}

/** Submit the open instance (submission=true) exactly once, then drop it. */
function closeScrobble(): void {
  if (scrobbleOpen == null) return;
  const { serverId, remoteId } = scrobbleOpen;
  scrobbleOpen = null;
  scrobbleNow(serverId, remoteId, true);
}

/** A TrackStarted made `item` current: cycle the scrobble onto it. */
function scrobbleStart(item: QueueItem): void {
  // Any previously started listen is over — same-id restarts included
  // (repeat-one wrap / replay = a new play of the same item).
  closeScrobble();
  if (item.remoteId == null) return; // local item: nothing to open
  scrobbleOpen = { itemId: item.id, remoteId: item.remoteId, serverId: item.serverId };
  scrobbleNow(item.serverId, item.remoteId, false);
}

// ── Local play stats (M6) ───────────────────────────────────────────────────
// The LOCAL twin of the scrobble section above, with the same one-instance
// guard and at the same lifecycle moments: `library_mark_played(path)` fires
// exactly once per started track, when playback LEAVES a local item — the
// next track's TrackStarted, an orphaned feed, EndOfQueue, or an explicit
// stop. Symmetry notes:
//  · scrobbleStart opens (now-playing) on start and submits on leave;
//    playedStart only opens on start — the local "submit" IS the leave-time
//    fire — so both flavors count each listen exactly once.
//  · Same-id restarts (repeat-one wrap / replay) close + reopen ⇒ two
//    plays, two marks — same as a second scrobble submission.
//  · clear() leaves the instance open (the engine keeps playing what it
//    holds; EndOfQueue or the next start closes it), matching scrobbling.
//  · Fire-and-forget outside the feed chain: a failed mark (e.g. a backend
//    without the command yet) only logs and never blocks feeds.

/** The open local listen, if the current track is a local file. */
let playedOpen: { itemId: string; path: string } | null = null;

function markPlayed(path: string): void {
  libraryMarkPlayed(path).catch((e) =>
    logLine(`✖ play stats: ${friendlyIpcError("library_mark_played", e)}`),
  );
}

/** Fire for the open local listen exactly once, then drop it. */
function closePlayed(): void {
  if (playedOpen == null) return;
  const { path } = playedOpen;
  playedOpen = null;
  markPlayed(path);
}

/** A TrackStarted made `item` current: cycle the local mark onto it. */
function playedStart(item: QueueItem): void {
  // Playback left whatever was open — close (fire) before considering this.
  closePlayed();
  if (item.remoteId != null) return; // remote: scrobbling owns the listen
  if (isStreamUrl(item.path)) return; // fed as a stream URL, not a file
  playedOpen = { itemId: item.id, path: item.path };
}

/** File-dialog flavor: tags are unknown, so the filename becomes the title. */
function itemFromPath(path: string): QueueItemMeta {
  const base = path.split(/[\\/]/).pop() ?? path;
  const dot = base.lastIndexOf(".");
  const title = dot > 0 ? base.slice(0, dot) : base;
  return { path, title, artist: "", durationSecs: null };
}

function rebuildShuffle(): void {
  shuffleOrder = items.map((_, i) => i);
  for (let i = shuffleOrder.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [shuffleOrder[i], shuffleOrder[j]] = [shuffleOrder[j], shuffleOrder[i]];
  }
}

function shuffleStep(i: number, delta: 1 | -1): number | null {
  const pos = shuffleOrder.indexOf(i);
  if (pos < 0) return null;
  const next = pos + delta;
  if (next >= 0 && next < shuffleOrder.length) return shuffleOrder[next];
  return repeat === "all"
    ? shuffleOrder[delta > 0 ? 0 : shuffleOrder.length - 1]
    : null;
}

/** Queue index the engine should play after `i` (null = let it run dry). */
function successorIndex(i: number): number | null {
  if (items.length === 0) return null;
  if (repeat === "one") return i; // self-feed → gapless repeat
  if (shuffle) return shuffleStep(i, 1);
  if (i + 1 < items.length) return i + 1;
  return repeat === "all" ? 0 : null;
}

/** Index for explicit next()/prev(): skips even in repeat=one, clamps at the ends (wraps only for repeat=all). */
function stepIndex(i: number, delta: 1 | -1): number {
  if (items.length === 0) return i;
  if (shuffle) return shuffleStep(i, delta) ?? i;
  let j = i + delta;
  if (j >= items.length) j = repeat === "all" ? 0 : items.length - 1;
  if (j < 0) j = repeat === "all" ? items.length - 1 : 0;
  return j;
}

/**
 * Queue indexes the engine should hold after `from`, in play order, up to
 * PREFETCH_AHEAD. Walks the same successor function as playback, so shuffle
 * and repeat behave identically; stops when the walk revisits a position
 * (a short queue under repeat=all would otherwise repeat forever).
 */
function upcomingIndexes(from: number): number[] {
  if (items.length === 0) return [];
  // repeat=one is its own successor; feed one self-copy so the repeat stays
  // gapless instead of relying on the EndOfQueue wrap.
  if (repeat === "one") return [from];
  // A one-item queue only has a successor under repeat=all.
  if (items.length === 1) return repeat === "all" ? [from] : [];
  const result: number[] = [];
  const seen = new Set<number>([from]);
  let at = from;
  while (result.length < PREFETCH_AHEAD) {
    const next = successorIndex(at);
    if (next == null || next >= items.length || seen.has(next)) break;
    seen.add(next);
    result.push(next);
    at = next;
  }
  return result;
}

/**
 * Make the engine's prefetch window match the queue's desired upcoming
 * tracks. Called after every mutation, toggle, and advance. No-ops when the
 * engine is not live or when the fed window already matches (compared by
 * item id, so list edits cannot desync it). The engine reconciles by key,
 * so re-declaring the surviving tail never re-opens it.
 */
function reconcileAhead(): void {
  if (!engineLive) return;
  let base: number | null = null;
  if (startPending && expectations.length > 0) {
    const front = items.findIndex((it) => it.id === expectations[0].id);
    if (front >= 0) base = front;
  } else if (playingItemId != null) {
    const idx = items.findIndex((it) => it.id === playingItemId);
    if (idx >= 0) base = idx;
  }
  if (base == null) {
    // Audible track orphaned (removed mid-play) or play still starting with
    // nothing known: keep whatever was already fed. Only a fully empty
    // engine chain falls back to the displayed position.
    if (expectations.length > 0) return;
    if (!startPending && currentIndex != null) base = currentIndex;
    else return;
  }

  const desired = upcomingIndexes(base);
  const targets = desired.map((i) => items[i]);
  const fedTail = startPending ? expectations.slice(1) : expectations;
  if (
    fedTail.length === targets.length &&
    fedTail.every((e, i) => e.id === targets[i].id)
  ) {
    return;
  }

  feed(async () => {
    const tracks = await Promise.all(
      targets.map(async (item) => {
        // Reuse the source that is already open in the engine; only new
        // window entries pay for a Navidrome/yt-dlp resolution.
        let path = windowSources.get(item.id);
        if (path == null) {
          path = await feedPath(item);
          windowSources.set(item.id, path);
        }
        return { key: item.id, path };
      }),
    );
    const keep = new Set(targets.map((item) => item.id));
    for (const id of [...windowSources.keys()]) {
      if (!keep.has(id)) windowSources.delete(id);
    }
    await invoke("audio_set_upcoming", { tracks });
  });
  const exps = desired.map((index) => ({ index, id: items[index].id }));
  expectations =
    startPending && expectations.length > 0 ? [expectations[0], ...exps] : exps;
}

function insertItems(metas: QueueItemMeta[], at: number): number {
  if (metas.length === 0) return items.length;
  const first = Math.max(0, Math.min(at, items.length));
  items.splice(first, 0, ...metas.map((m) => ({ ...m, id: newId() })));
  // First add selects without playing; later inserts keep the current item.
  if (currentIndex == null) currentIndex = 0;
  else if (currentIndex >= first) currentIndex += metas.length;
  if (shuffle) rebuildShuffle();
  reconcileAhead();
  return first;
}

function play(index: number): void {
  if (index < 0 || index >= items.length) return;
  const item = items[index];
  const session = ++playSession;
  currentIndex = index;
  playingItemId = item.id;
  engineLive = true;
  startPending = true;
  expectations = [{ index, id: item.id }];
  feed(async () => {
    await invoke("audio_play", {
      key: item.id,
      path: await feedPath(item),
    });
    // A newer play supersedes this one; its own feed is queued after this.
    if (session !== playSession) return;
  });
  // Queue the window behind the play command in the same serialized feed
  // chain. The engine opens it while the Play target starts; re-declaring
  // the surviving keys after every advance is intentionally cheap.
  reconcileAhead();
}

function next(): void {
  if (items.length === 0) return;
  if (currentIndex == null) {
    play(0);
    return;
  }
  const j = stepIndex(currentIndex, 1);
  if (j !== currentIndex) play(j); // clamped at the end: no-op (Winamp-like)
}

function prev(): void {
  if (items.length === 0) return;
  if (currentIndex == null) {
    play(0);
    return;
  }
  play(stepIndex(currentIndex, -1)); // clamped at the start: restart current
}

// ── persistence (player window only): the queue survives restarts ───────────
// Restored at module init; saved by the bus's broadcast effect after every
// mutation. Durations come back null and refill on first play. Stored in the
// player webview's localStorage — safe because ONLY the player window owns
// (mutates) this module; other windows render bus snapshots.

const PERSIST_KEY = "navinamp.queue.v1";
/** Above this, the JSON (~250 B/item) no longer fits webview localStorage. */
const MAX_PERSISTED_ITEMS = 10_000;
let persistSkipWarned = false;

export function persistQueue(snapshot: {
  items: QueueItem[];
  currentIndex: number | null;
  shuffle: boolean;
  repeat: RepeatMode;
}): void {
  if (!IS_PLAYER) return;
  if (snapshot.items.length > MAX_PERSISTED_ITEMS) {
    // A 50k-track queue is ~10 MB and exceeds the storage quota; skip the
    // stringify cost. Leave the last good snapshot in place — a transient
    // PLAY ALL burst must not destroy a previously saved queue.
    // (Warn once per session; a later shrink below the cap saves normally.)
    if (!persistSkipWarned) {
      persistSkipWarned = true;
      console.warn(
        `[navinamp] queue has ${snapshot.items.length} items; ` +
          `persistence is limited to ${MAX_PERSISTED_ITEMS}`,
      );
    }
    return;
  }
  try {
    localStorage.setItem(PERSIST_KEY, JSON.stringify(snapshot));
  } catch {
    // storage unavailable — the queue just lives for this session
  }
}

if (IS_PLAYER) {
  try {
    const raw = localStorage.getItem(PERSIST_KEY);
    if (raw) {
      const saved = JSON.parse(raw) as {
        items?: QueueItem[];
        currentIndex?: number | null;
        shuffle?: boolean;
        repeat?: RepeatMode;
      };
      // Loads accept any valid saved queue: the save cap only limits writes,
      // so an older/larger snapshot is never truncated on restore.
      if (Array.isArray(saved.items)) {
        items = saved.items.filter(
          (it) =>
            it &&
            typeof it.id === "string" &&
            typeof it.path === "string" &&
            typeof it.title === "string",
        );
        currentIndex =
          typeof saved.currentIndex === "number" &&
          saved.currentIndex >= 0 &&
          saved.currentIndex < items.length
            ? saved.currentIndex
            : items.length > 0
              ? 0
              : null;
        shuffle = saved.shuffle === true;
        if (saved.repeat === "all" || saved.repeat === "one") repeat = saved.repeat;
      }
    }
  } catch {
    // corrupt saved state — start fresh
  }
  void repairRestoredText();
}

/**
 * Repair legacy encodings in the persisted queue's metadata (best effort).
 * The queue stores snapshots, so items saved before charset recovery would
 * keep garbled titles after the library rows were healed. Deduplicated and
 * chunked: a huge queue usually has far fewer unique title/artist strings.
 */
async function repairRestoredText(): Promise<void> {
  if (items.length === 0) return;
  const titles = Array.from(new Set(items.map((item) => item.title)));
  const artists = Array.from(new Set(items.map((item) => item.artist)));

  const repair = async (texts: string[]): Promise<string[]> => {
    const fixed: string[] = [];
    for (let i = 0; i < texts.length; i += 4000) {
      const batch = texts.slice(i, i + 4000);
      fixed.push(...(await invoke<string[]>("repair_tag_texts", { texts: batch })));
    }
    return fixed;
  };

  // The heuristic can occasionally pick a wrong codepage for accented Latin
  // text. Only persist a rewrite that actually produces Cyrillic; the queue
  // snapshot is otherwise a permanent copy, unlike the re-readable DB rows.
  const accept = (original: string, repaired: string): string =>
    repaired !== original && /[\u0400-\u04FF]/.test(repaired)
      ? repaired
      : original;

  try {
    const [fixedTitles, fixedArtists] = await Promise.all([
      repair(titles),
      repair(artists),
    ]);
    const titleMap = new Map<string, string>();
    titles.forEach((title, i) =>
      titleMap.set(title, accept(title, fixedTitles[i] ?? title)),
    );
    const artistMap = new Map<string, string>();
    artists.forEach((artist, i) =>
      artistMap.set(artist, accept(artist, fixedArtists[i] ?? artist)),
    );
    let changed = false;
    const next = items.map((item) => {
      const title = titleMap.get(item.title) ?? item.title;
      const artist = artistMap.get(item.artist) ?? item.artist;
      if (title === item.title && artist === item.artist) return item;
      changed = true;
      return { ...item, title, artist };
    });
    if (changed) items = next;
  } catch {
    // backend unavailable (browser preview / older build) — keep raw text
  }
}

export const queue = {
  get items(): QueueItem[] {
    return items;
  },
  get currentIndex(): number | null {
    return currentIndex;
  },
  get shuffle(): boolean {
    return shuffle;
  },
  get repeat(): RepeatMode {
    return repeat;
  },
  get currentItem(): QueueItem | null {
    return currentIndex == null ? null : (items[currentIndex] ?? null);
  },
  get totalSecs(): number {
    return items.reduce((sum, it) => sum + (it.durationSecs ?? 0), 0);
  },
  get formattedTotal(): string {
    return formatTime(this.totalSecs);
  },
  /** Fed-but-unstarted feeds, as queue indexes resolved by id (-1 = gone). */
  get expectationIndexes(): number[] {
    return expectations.map((e) => items.findIndex((it) => it.id === e.id));
  },

  /** Route the app's log line into feed-error reporting. */
  setLog(fn: (line: string) => void): void {
    log = fn;
  },

  /**
   * Append file-dialog paths (titles guessed from filenames; only audio
   * extensions are kept). Returns the index of the first added item. The
   * first add into an empty queue sets currentIndex=0 WITHOUT playing.
   */
  add(paths: string[]): number {
    return insertItems(
      paths.filter(isAudioPath).map(itemFromPath),
      items.length,
    );
  },

  /**
   * M4 extension point: enqueue fully-tagged tracks from the library
   * (scanner / Navidrome) without a file dialog. Same insertion rules as
   * add(); pass an explicit insertion index for "play next" placement.
   */
  addFromLibrary(metas: QueueItemMeta[]): number {
    return insertItems(metas, items.length);
  },

  /** Insert one path at `index` (default: right after the current item). */
  addNext(path: string, index?: number): void {
    const at =
      index ?? (currentIndex == null ? items.length : Math.min(currentIndex + 1, items.length));
    if (isAudioPath(path)) insertItems([itemFromPath(path)], at);
  },

  /**
   * Append a validated playlist URL. URL rows bypass the file-extension
   * allow-list; the canonical link is persisted (YouTube re-resolves its
   * direct media URL at feed time).
   */
  addUrl(meta: QueueItemMeta): number {
    if (!isStreamUrl(meta.path)) {
      // Never echo the URL itself: playlist links may carry credentials.
      logLine("✖ playlist URL rejected: only http(s) links are supported");
      return items.length;
    }
    return insertItems([meta], items.length);
  },

  remove(index: number): void {
    if (index < 0 || index >= items.length) return;
    const wasCurrent = index === currentIndex;
    items.splice(index, 1);
    if (currentIndex != null) {
      if (wasCurrent) {
        // Engine keeps playing the removed file until its chain moves on;
        // the display slides to the item that took its place (or empties).
        currentIndex = items.length > 0 ? Math.min(index, items.length - 1) : null;
      } else if (currentIndex > index) {
        currentIndex -= 1;
      }
    }
    if (wasCurrent) playingItemId = null; // audible track is now an orphan
    if (shuffle) rebuildShuffle();
    reconcileAhead();
  },

  /** Move an item to final index `to`, keeping currentIndex on the same item. */
  move(from: number, to: number): void {
    if (from === to) return;
    if (from < 0 || from >= items.length) return;
    if (to < 0 || to >= items.length) return;
    const currentId = currentIndex != null ? items[currentIndex].id : null;
    const [moved] = items.splice(from, 1);
    items.splice(to, 0, moved);
    if (currentId != null) currentIndex = items.findIndex((it) => it.id === currentId);
    if (shuffle) rebuildShuffle();
    reconcileAhead();
  },

  /**
   * Empty the list. The engine keeps playing the current track, but the
   * lookahead window is cancelled so removed items are not played ahead.
   */
  clear(): void {
    items = [];
    currentIndex = null;
    expectations = [];
    startPending = false;
    playingItemId = null;
    shuffleOrder = [];
    windowSources.clear();
    if (engineLive) {
      feed(() => invoke("audio_set_upcoming", { tracks: [] }));
    }
  },

  /** Double-click / PLAY-on-stopped: feed `index` as a fresh play. */
  play,

  /** Skips forward (shuffle/repeat aware; clamped, never self). */
  next,

  /** Skips backward (shuffle/repeat aware; restarts current at the start). */
  prev,

  toggleShuffle(): void {
    shuffle = !shuffle;
    if (shuffle) rebuildShuffle();
    reconcileAhead(); // the desired successor may change → re-feed pending
  },

  cycleRepeat(): void {
    repeat = repeat === "off" ? "all" : repeat === "all" ? "one" : "off";
    reconcileAhead(); // e.g. →"one" re-feeds the current track as next
  },

  /**
   * The app stopped the engine explicitly (STOP button / debug UI): the
   * engine queue is gone. currentIndex stays so PLAY can restart there.
   */
  onStopped(): void {
    engineLive = false;
    expectations = [];
    startPending = false;
    playingItemId = null;
    windowSources.clear();
    closeScrobble(); // the audible remote listen is over (submission=true)
    closePlayed(); // …and the local listen is over (library_mark_played)
  },

  /**
   * Feed one engine event through the correlation protocol. Returns a log
   * line for queue-owned transitions, or null when the raw event should be
   * logged as-is (TrackFinished, Error, or anything we did not feed).
   */
  handleEngineEvent(e: EngineEvent): string | null {
    switch (e.type) {
      case "TrackStarted": {
        engineLive = true;
        if (expectations.length === 0) return null; // not our feed
        const exp = expectations.shift()!;
        if (startPending) startPending = false;
        const idx = items.findIndex((it) => it.id === exp.id);
        if (idx >= 0) {
          currentIndex = idx;
          playingItemId = exp.id;
          scrobbleStart(items[idx]); // now-playing on remote, closes the previous
          playedStart(items[idx]); // local twin: closes the previous, marks on leave
          if (e.durationSecs != null && items[idx].durationSecs == null) {
            items[idx].durationSecs = e.durationSecs;
          }
          reconcileAhead();
          return `▶ queue[${idx}] started${e.durationSecs != null ? ` (${formatTime(e.durationSecs)})` : ""}`;
        }
        // Fed item no longer in the list: it plays out, position unchanged.
        // Its remoteId is unreachable by then, so an open listen is closed
        // (submitted) here rather than leaked.
        playingItemId = null;
        closeScrobble();
        closePlayed();
        reconcileAhead();
        return `▶ started a track no longer in the list (fed #${exp.index})`;
      }
      case "EndOfQueue": {
        if (!engineLive) return null;
        expectations = [];
        startPending = false;
        windowSources.clear();
        // The audible track ran dry — submit its listen now; a repeat=all
        // wrap re-opens (now-playing) from the wrap's own TrackStarted.
        closeScrobble();
        closePlayed();
        if (repeat === "all" && items.length > 0) {
          play(0);
          return "↻ end of queue — repeat all from queue[0]";
        }
        engineLive = false;
        return "= end of queue";
      }
      default:
        return null;
    }
  },
};
