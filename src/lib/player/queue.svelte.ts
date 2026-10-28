/**
 * Play queue + engine correlation (M3).
 *
 * ── Engine correlation protocol ("expectations") ────────────────────────────
 * The audio engine owns its playback queue and only reports progress through
 * `audio-event`s; its track ids are engine-internal u64s. The UI therefore
 * correlates by FEED ORDER:
 *
 *  · `expectations` lists every track we fed to the engine (via audio_play /
 *    audio_enqueue) that has not started yet, oldest first — the front entry
 *    is the queue item the next TrackStarted is expected to belong to.
 *  · TrackStarted shifts the front entry; the shifted item becomes
 *    currentIndex. This is the ONLY event that moves the position — it is
 *    how a gapless engine-side advance updates the UI.
 *  · TrackFinished never moves the position: audio_play REPLACES the engine
 *    queue and emits TrackFinished for the replaced track, so "finished"
 *    does not imply "advance on our side".
 *  · audio_enqueue pre-buffers exactly ONE next track (replacing any pending
 *    one, and starting playback if the engine is idle), so we keep at most
 *    one ahead: after every TrackStarted advance we feed the successor of
 *    the new current index (reconcileAhead), guarded by the expectation
 *    list so the same position is never fed twice.
 *
 * Edge cases:
 *  · Play-replace during gapless — play(i) resets the engine queue and our
 *    expectations to [i (, next)]. A straggler TrackStarted from the replaced
 *    pipeline then just consumes the front entry; since exactly one entry is
 *    consumed per TrackStarted and expectations always mirrors
 *    fed-but-unstarted tracks, the position converges (worst case one
 *    transient mis-highlight).
 *  · Enqueue at queue end — no successor exists, nothing is fed; the engine
 *    runs dry and emits EndOfQueue: repeat=all wraps via play(0), anything
 *    else is a stop state (currentIndex stays, engineLive=false).
 *  · repeat=one feeds the current track as its own successor, making repeats
 *    gapless; the EndOfQueue wrap is then only a fallback.
 *
 * Remote (M5): items synced from Navidrome carry `path: "navidrome://<id>"`
 * plus `remoteId`. Inside the serialized feed chain `feedPath` translates
 * them into a fresh stream URL (they can be time-limited, so every feed
 * resolves its own); a failed resolution rejects into the existing
 * feed-error log and the item simply never plays (like a bad file). The
 * same TrackStarted/EndOfQueue/stop transitions drive Subsonic scrobbling —
 * see the scrobble section below the feed helpers.
 *
 * Entries carry the item id next to the fed index so list edits (remove /
 * move / insert) repair indexes by identity — a bare index would go stale
 * after a splice. If a fed item is removed from the list the engine still
 * plays its file out once (audio_enqueue can replace a pending next, but
 * nothing can cancel it); the UI simply does not move for it.
 */

import { invoke } from "@tauri-apps/api/core";

import { navidromeScrobble, navidromeStreamUrl } from "../navidrome/ipc";

export type RepeatMode = "off" | "all" | "one";

export interface QueueItem {
  /** Stable UI identity (survives reorders; the engine has its own u64 ids). */
  id: string;
  /**
   * Local file path, or `navidrome://<remoteId>` for synced remote rows.
   * Remote items resolve their real stream URL at feed time (feedPath).
   */
  path: string;
  title: string;
  artist: string;
  durationSecs: number | null;
  /**
   * Navidrome track id — present only on remote items; drives stream-URL
   * resolution and scrobbling. `null`/undefined ⇒ a local file.
   */
  remoteId?: string;
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
/** Distinguishes our feed from a newer play() when feeds resolve. */
let playSession = 0;
let idCounter = 0;
let log: ((line: string) => void) | null = null;

export function isAudioPath(path: string): boolean {
  const dot = path.lastIndexOf(".");
  const ext = dot >= 0 ? path.slice(dot + 1).toLowerCase() : "";
  return AUDIO_EXTENSIONS.has(ext);
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
 * `navidrome://<remoteId>` and must be translated into a stream URL first —
 * URLs can be time-limited, so each feed resolves its own instead of
 * caching. A failed resolution rejects inside the serialized feed chain and
 * lands in the existing feed-error log above; the item is skipped exactly
 * like a file the engine cannot open.
 */
async function feedPath(item: QueueItem): Promise<string> {
  if (item.remoteId == null) return item.path;
  return navidromeStreamUrl(item.remoteId);
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
let scrobbleOpen: { itemId: string; remoteId: string } | null = null;

function scrobbleNow(remoteId: string, submission: boolean): void {
  navidromeScrobble(remoteId, submission).catch((e) =>
    logLine(`✖ scrobble failed (${submission ? "submit" : "now-playing"} ${remoteId}): ${e}`),
  );
}

/** Submit the open instance (submission=true) exactly once, then drop it. */
function closeScrobble(): void {
  if (scrobbleOpen == null) return;
  const { remoteId } = scrobbleOpen;
  scrobbleOpen = null;
  scrobbleNow(remoteId, true);
}

/** A TrackStarted made `item` current: cycle the scrobble onto it. */
function scrobbleStart(item: QueueItem): void {
  // Any previously started listen is over — same-id restarts included
  // (repeat-one wrap / replay = a new play of the same item).
  closeScrobble();
  if (item.remoteId == null) return; // local item: nothing to open
  scrobbleOpen = { itemId: item.id, remoteId: item.remoteId };
  scrobbleNow(item.remoteId, false);
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
 * The pending "next" slot: the last entry while a play's TrackStarted is
 * still outstanding, the single front entry afterwards.
 */
function pendingExpectation(): Expectation | null {
  if (startPending) return expectations.length > 1 ? expectations[expectations.length - 1] : null;
  return expectations.length > 0 ? expectations[0] : null;
}

/**
 * Make the engine's pre-buffered next match the queue's desired successor.
 * Called after every mutation, toggle, and advance. No-ops when the engine
 * is not live, when the desired track was already fed (compared by id, so
 * list edits cannot desync it), or when there is nothing to feed (a fed
 * pending cannot be cancelled — it plays out once).
 */
function reconcileAhead(): void {
  if (!engineLive) return;
  const curIdx =
    playingItemId != null ? items.findIndex((it) => it.id === playingItemId) : -1;
  const pending = pendingExpectation();
  let desired: number | null;
  if (curIdx >= 0) {
    desired = successorIndex(curIdx);
  } else if (pending != null) {
    // Audible track orphaned (removed mid-play) or play still starting:
    // keep whatever was already fed as the pending next.
    desired = items.findIndex((it) => it.id === pending.id);
    if (desired < 0) desired = null;
  } else {
    // Orphaned with nothing pending: keep the chain alive from the
    // displayed position so the queue does not dead-end.
    desired = currentIndex != null ? successorIndex(currentIndex) : null;
  }
  if (desired == null || desired >= items.length) return;
  if (pending != null && pending.id === items[desired].id) return;
  const target = items[desired];
  feed(async () => {
    await invoke("audio_enqueue", { path: await feedPath(target) });
  });
  if (startPending && expectations.length > 0) {
    // Preserve the play-start front entry; the fed next follows it.
    expectations = [expectations[0], { index: desired, id: target.id }];
  } else {
    expectations = [{ index: desired, id: target.id }];
  }
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
  const next = successorIndex(index);
  const nextId = next != null && next < items.length ? items[next].id : null;
  const session = ++playSession;
  currentIndex = index;
  playingItemId = item.id;
  engineLive = true;
  startPending = true;
  expectations = [{ index, id: item.id }];
  feed(async () => {
    await invoke("audio_play", { path: await feedPath(item) });
    // Superseded by a newer play, or the TrackStarted already arrived and
    // reconcileAhead owns ahead-feeding from here.
    if (session !== playSession || nextId == null) return;
    if (!startPending || expectations.length !== 1) return;
    const target = items.find((it) => it.id === nextId);
    if (!target) return;
    expectations = [expectations[0], { index: items.indexOf(target), id: target.id }];
    await invoke("audio_enqueue", { path: await feedPath(target) });
  });
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

  /** Empty the list. The engine finishes whatever it already holds. */
  clear(): void {
    items = [];
    currentIndex = null;
    expectations = [];
    startPending = false;
    playingItemId = null;
    shuffleOrder = [];
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
    closeScrobble(); // the audible remote listen is over (submission=true)
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
        reconcileAhead();
        return `▶ started a track no longer in the list (fed #${exp.index})`;
      }
      case "EndOfQueue": {
        if (!engineLive) return null;
        expectations = [];
        startPending = false;
        // The audible track ran dry — submit its listen now; a repeat=all
        // wrap re-opens (now-playing) from the wrap's own TrackStarted.
        closeScrobble();
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
