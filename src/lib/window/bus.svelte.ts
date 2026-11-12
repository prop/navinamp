/**
 * Cross-window bus (M6b): the window-manager store plus the queue-intent /
 * queue-snapshot protocol that keeps the player window the single owner of
 * playback state.
 *
 * ── Ownership model ────────────────────────────────────────────────────────
 * "main" (player) OWNS the queue module, the engine-event listener, the
 * audio pollers and the spectrum poller. The other windows are views:
 *
 *  · MUTATIONS — non-player windows emit `queue-intent` (the backend relays
 *    it); intents are the queue's public method calls serialized. In the
 *    player window `queueIntent()` executes DIRECTLY against the local
 *    queue module — no event round-trip, so the skin's buttons stay in
 *    lock-step with its rendering.
 *  · SNAPSHOTS — the player broadcasts `queue-state` after EVERY queue
 *    mutation (an effect deep-reads the queue, so splices, duration patches
 *    from TrackStarted, toggles and index moves all trigger it) and on
 *    `queue-state-request`, which non-player windows emit on mount and
 *    whenever they become visible again (heals a missed snapshot, e.g. a
 *    player window that was still loading when the view first asked).
 *
 * Every frontend→frontend event is emitted broadcast-style and every
 * listener guards by payload fields (`from: "main"` / `from !== own label`),
 * so the protocol is correct whether the backend relays events to all
 * windows or filters out the sender.
 *
 * Degradation: before the backend lands, `windows_get` fails (the store
 * stays empty → default visibility/layout), and intents from non-player
 * windows go nowhere — the player view itself keeps working standalone.
 */

import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { IS_PLAYER, LABEL, PLAYER_LABEL, type WindowLabel } from "./identity";
import { persistPlayback, persistQueue, queue, queueReady } from "../player/queue.svelte";
import type { QueueItem, QueueItemMeta, RepeatMode } from "../player/queue.svelte";
import {
  DEFAULT_SKIN_THEME,
  themeFromSkinParams,
  type SkinTheme,
} from "../skin/theme";

// ── window-manager IPC (frozen M6b contract) ───────────────────────────────

/** One OS window's state, as reported by `windows_get` / `windows-changed`. */
export interface WinInfo {
  visible: boolean;
  layout: "normal" | "shade";
  /** Kept above other applications' windows (persisted). */
  onTop: boolean;
  x: number;
  y: number;
  w: number;
  h: number;
}

export function windowsGet(): Promise<Record<string, WinInfo>> {
  return invoke<Record<string, WinInfo>>("windows_get");
}

export function windowSetVisible(label: WindowLabel, visible: boolean): Promise<void> {
  return invoke<void>("window_set_visible", { label, visible });
}

export function windowToggleLayout(label: WindowLabel): Promise<void> {
  return invoke<void>("window_toggle_layout", { label });
}

/** Keep `label` above all other windows (session-level option). */
export function windowSetAlwaysOnTop(label: WindowLabel, on: boolean): Promise<void> {
  return invoke<void>("window_set_always_on_top", { label, on });
}

export function windowSetSize(label: WindowLabel, w: number, h: number): Promise<void> {
  return invoke<void>("window_set_size", { label, w, h });
}

/**
 * Deduplicating window-size writer. Every window derives its desired size
 * from a different source (skin layout, measured content, scaled natural
 * box), but they all go through this one guard: a `windows-changed` event
 * cannot loop back into another identical `window_set_size` call.
 */
const lastWindowSizes = new Map<WindowLabel, { w: number; h: number }>();

export function sizeWindowTo(label: WindowLabel, w: number, h: number): void {
  const width = Math.ceil(w);
  const height = Math.ceil(h);
  if (
    !Number.isFinite(width) ||
    !Number.isFinite(height) ||
    width <= 0 ||
    height <= 0
  ) {
    return;
  }
  const prev = lastWindowSizes.get(label);
  if (prev?.w === width && prev.h === height) return;
  const entry = { w: width, h: height };
  lastWindowSizes.set(label, entry);
  windowSetSize(label, width, height).catch(() => {
    // Don't remember a failed request; a later observer/effect can retry it.
    if (lastWindowSizes.get(label) === entry) lastWindowSizes.delete(label);
  });
}

// ── UI scale (global, persisted, broadcast to every window) ────────────────

const UI_SCALE_KEY = "navinamp.ui.scale";

/**
 * Dev-tab choices. Integer scales keep the pixel skin crisp; 1.25/1.5 are
 * for scaled macOS displays. 2× is the ceiling: the library is 652×518
 * natural, so larger values overflow most laptop screens. The OS window is
 * sized in logical pixels and the skin is CSS-transformed, so a 2× display
 * effectively device-pixel-scales the bitmap again.
 */
export const UI_SCALE_PRESETS = [1, 1.25, 1.5, 2] as const;

function normalizeUiScale(value: number): number {
  if (!Number.isFinite(value)) return 1;
  const clamped = Math.min(
    UI_SCALE_PRESETS[UI_SCALE_PRESETS.length - 1],
    Math.max(UI_SCALE_PRESETS[0], value),
  );
  return UI_SCALE_PRESETS.reduce((best, preset) =>
    Math.abs(preset - clamped) < Math.abs(best - clamped) ? preset : best,
  );
}

function storedUiScale(): number {
  try {
    const raw = localStorage.getItem(UI_SCALE_KEY);
    if (raw == null) return 1;
    return normalizeUiScale(Number.parseFloat(raw));
  } catch {
    return 1; // storage-less webview
  }
}

/** Current global scale; every window reads this reactive store. */
export const uiScale = $state<{ value: number }>({ value: storedUiScale() });

/**
 * Set the global UI scale. Every window persists the value; the originating
 * window also broadcasts it (receivers pass `broadcast = false`).
 */
export function setUiScale(value: number, broadcast = true): void {
  const next = normalizeUiScale(value);
  if (uiScale.value === next) return;
  uiScale.value = next;
  try {
    localStorage.setItem(UI_SCALE_KEY, String(next));
  } catch {
    // storage-less webview: session-only
  }
  if (broadcast) void emit("ui-scale", { scale: next });
}

/** Alt+S step through the presets (works from any window). */
export function cycleUiScale(): void {
  const i = UI_SCALE_PRESETS.findIndex((preset) => preset === uiScale.value);
  setUiScale(UI_SCALE_PRESETS[(i + 1) % UI_SCALE_PRESETS.length]);
}

// ── queue intents (the queue's public methods, serialized) ─────────────────

export type QueueIntent =
  | { type: "play"; index: number }
  | { type: "add"; paths: string[] }
  | { type: "addUrl"; meta: QueueItemMeta }
  | { type: "addFromLibrary"; metas: QueueItemMeta[] }
  /** Enqueue right after the current item ("play next"). */
  | { type: "addFromLibraryNext"; metas: QueueItemMeta[] }
  /** Append and start the first appended item, resolved by the player. */
  | { type: "addFromLibraryAndPlay"; metas: QueueItemMeta[] }
  | { type: "replaceAll"; metas: QueueItemMeta[] }
  | { type: "addNext"; path: string; index?: number }
  | { type: "remove"; index: number }
  | { type: "move"; from: number; to: number }
  /** Move `index` to just after the current item ("play next"). */
  | { type: "moveNext"; index: number }
  | { type: "clear" }
  | { type: "next" }
  | { type: "prev" }
  | { type: "toggleShuffle" }
  | { type: "cycleRepeat" }
  | { type: "onStopped" };

/** The queue snapshot the player broadcasts after every mutation. */
export interface QueueSnapshot {
  items: QueueItem[];
  currentIndex: number | null;
  shuffle: boolean;
  repeat: RepeatMode;
}

/** `seekBy` carries a signed delta in `secs`; `seekTo` an absolute position. */
export type MediaCommand =
  | "play"
  | "pause"
  | "stop"
  | "next"
  | "prev"
  | "seekBy"
  | "seekTo";

/** Apply one intent to the local queue — only meaningful in the player. */
export function applyQueueIntent(intent: QueueIntent): void {
  switch (intent.type) {
    case "play":
      queue.play(intent.index);
      break;
    case "add":
      queue.add(intent.paths);
      break;
    case "addUrl":
      queue.addUrl(intent.meta);
      break;
    case "addFromLibrary":
      queue.addFromLibrary(intent.metas);
      break;
    case "addFromLibraryNext":
      queue.addNextFromLibrary(intent.metas);
      break;
    case "addFromLibraryAndPlay": {
      const first = queue.addFromLibrary(intent.metas);
      queue.play(first);
      break;
    }
    case "replaceAll":
      queue.clear();
      queue.addFromLibrary(intent.metas);
      break;
    case "addNext":
      queue.addNext(intent.path, intent.index);
      break;
    case "remove":
      queue.remove(intent.index);
      break;
    case "move":
      queue.move(intent.from, intent.to);
      break;
    case "moveNext":
      queue.moveNext(intent.index);
      break;
    case "clear":
      queue.clear();
      break;
    case "next":
      queue.next();
      break;
    case "prev":
      queue.prev();
      break;
    case "toggleShuffle":
      queue.toggleShuffle();
      break;
    case "cycleRepeat":
      queue.cycleRepeat();
      break;
    case "onStopped":
      queue.onStopped();
      break;
  }
}

/**
 * Mutate the queue from any window. Player: plain local call (no round-trip).
 * Elsewhere: `queue-intent { intent, from: label }` for the player to execute.
 */
export function queueIntent(intent: QueueIntent): void {
  if (IS_PLAYER) {
    applyQueueIntent(intent);
    return;
  }
  void emit("queue-intent", { intent, from: LABEL });
}

/**
 * "Play now" from a library view: the player appends the metas and starts the
 * first appended item itself. The index must never be computed from a
 * cross-window snapshot — snapshots are debounced, so a stale length would
 * start an already-queued track (or index 0 on a view without a snapshot).
 */
export function queuePlayNow(metas: QueueItemMeta[]): void {
  if (metas.length === 0) return;
  queueIntent({ type: "addFromLibraryAndPlay", metas });
}

/** Replace the playlist editor/queue contents with `metas`. */
export function queueReplaceAll(metas: QueueItemMeta[]): void {
  if (metas.length === 0) return;
  queueIntent({ type: "replaceAll", metas });
}

/** "Play next" from a library view: enqueue right after the current item. */
export function queuePlayNext(metas: QueueItemMeta[]): void {
  if (metas.length === 0) return;
  queueIntent({ type: "addFromLibraryNext", metas });
}

// ── reactive stores ────────────────────────────────────────────────────────

/** Window-manager state, kept fresh by `windows-changed`. */
export const windowsStore = $state<{ byLabel: Record<string, WinInfo> }>({ byLabel: {} });

/**
 * Latest queue snapshot — meaningful only in NON-player windows (the player
 * renders its own queue and never consumes snapshots).
 */
export const queueSnapshot = $state<{ value: QueueSnapshot | null }>({ value: null });

/**
 * Loaded skin's playlist theme (pledit.txt colors, classic-green fallback).
 * The player broadcasts it after every skin load; non-player windows keep
 * the default until then. Same request/re-heal protocol as the queue
 * snapshot, so a playlist window that mounts before the player's skin load
 * still catches up.
 */
export const skinTheme = $state<{ value: SkinTheme }>({ value: DEFAULT_SKIN_THEME });

/**
 * Labels of windows that currently hold focus. Every window reports its own
 * focus/blur (plus an answer to `app-focus-request`) and every window
 * aggregates the set, so `appFocused()` is accurate wherever it is called —
 * the player gates desktop notifications on it. Native `emit` broadcasts to
 * every webview and the updates are idempotent, so this needs no backend
 * relay (unlike queue intents).
 */
export const appFocus = $state<{ labels: Set<string> }>({ labels: new Set() });

/** True while any NaVinaMP window is focused (player notification gate). */
export function appFocused(): boolean {
  return appFocus.labels.size > 0;
}

function setFocused(label: string, on: boolean): void {
  const current = appFocus.labels;
  if (current.has(label) === on) return;
  const next = new Set(current);
  if (on) next.add(label);
  else next.delete(label);
  appFocus.labels = next;
}

function ingestWindows(map: Record<string, WinInfo> | undefined): void {
  if (map) windowsStore.byLabel = map;
}

/** WinInfo for a label (undefined until the first `windows_get` resolves). */
export function winInfo(label: WindowLabel): WinInfo | undefined {
  return windowsStore.byLabel[label];
}

/** A window's visibility; defaults to visible until the manager reports. */
export function windowIsVisible(label: WindowLabel): boolean {
  return winInfo(label)?.visible ?? true;
}

/** A window's current layout (defaults to "normal"). */
export function layoutMode(label: WindowLabel): "normal" | "shade" {
  return winInfo(label)?.layout ?? "normal";
}

/** The player window's current layout (defaults to "normal"). */
export function playerLayoutMode(): "normal" | "shade" {
  return layoutMode("main");
}

// ── media commands (media keys / DEV transport) ────────────────────────────

let mediaExecutor: ((command: MediaCommand, secs?: number) => void) | null = null;

/**
 * Player-side hook: how to run media commands. Wired to the SAME executor
 * as the skin's action buttons so PLAY/PAUSE keep their resume-vs-restart
 * semantics no matter where the command originated.
 */
export function setMediaCommandExecutor(
  fn: (command: MediaCommand, secs?: number) => void,
): void {
  mediaExecutor = fn;
}

/** Frontend→frontend transport command (the library's DEV block uses it). */
export function emitMediaCommand(command: MediaCommand, secs?: number): void {
  void emit("media-command", secs == null ? { command } : { command, secs });
}

// ── snapshot plumbing ──────────────────────────────────────────────────────

/** Plain copy of the queue state; deep-reads every tracked field. */
function readSnapshot(): QueueSnapshot {
  return {
    items: queue.items.map((it) => ({ ...it })),
    currentIndex: queue.currentIndex,
    shuffle: queue.shuffle,
    repeat: queue.repeat,
  };
}

function broadcastQueueState(): void {
  void emit("queue-state", { from: "main", snapshot: readSnapshot() });
}

// ── skin theme plumbing ─────────────────────────────────────────────────────

/** The theme the player last published (starts as the classic default). */
let publishedTheme: SkinTheme = DEFAULT_SKIN_THEME;

function broadcastSkinTheme(): void {
  void emit("skin-theme", { from: "main", theme: publishedTheme });
}

/**
 * Player-side hook: publish the loaded skin's pledit colors to every window
 * (call after each skin load, including the mock fallback).
 */
export function publishSkinTheme(params: Record<string, string> | undefined): void {
  publishedTheme = themeFromSkinParams(params);
  broadcastSkinTheme();
}

// ── initialization ─────────────────────────────────────────────────────────

let initialized = false;

/**
 * Wire this window into the bus. Call once during App initialization (the
 * player branch creates a reactive effect, so a component context must be
 * active). Idempotent — safe under HMR remounts.
 */
export function initWindowBus(): void {
  if (initialized) return;
  initialized = true;

  // Window-manager state — every window tracks it (visibility toggles,
  // layout mode, geometry).
  void windowsGet().then(ingestWindows, () => undefined); // no backend yet → defaults
  void listen<{ windows: Record<string, WinInfo> }>("windows-changed", (ev) =>
    ingestWindows(ev.payload?.windows),
  );

  // App-wide focus: every window reports its own transitions and answers
  // `app-focus-request` (the player asks once on boot to heal windows that
  // were already focused before its listeners existed — same re-heal idea
  // as the queue snapshot's). `lastFocus` dedupes DOM focus/blur noise.
  let lastFocus: boolean | null = null;
  const reportFocus = () => {
    const on = document.hasFocus();
    if (on === lastFocus) return;
    lastFocus = on;
    setFocused(LABEL, on);
    void emit("app-focus", { from: LABEL, focused: on });
  };
  window.addEventListener("focus", reportFocus);
  window.addEventListener("blur", reportFocus);
  void listen("app-focus-request", () => {
    lastFocus = null; // force a re-broadcast of the current state
    reportFocus();
  });
  reportFocus();

  // Other windows' focus state (our own comes from `reportFocus` above).
  // Idempotent updates, so every native copy is safe to process in every
  // window — `appFocused()` stays correct wherever it is read.
  void listen<{ from: string; focused: boolean }>("app-focus", (ev) => {
    const p = ev.payload;
    if (!p || p.from === PLAYER_LABEL) return;
    setFocused(p.from, p.focused);
  });

  // Global UI scale: the media library's DEV tab changes it, every window
  // (including this one) resizes its content and OS window from the store.
  void listen<{ scale: number }>("ui-scale", (ev) => {
    const scale = ev.payload?.scale;
    if (typeof scale === "number" && Number.isFinite(scale)) {
      setUiScale(scale, false);
    }
  });

  if (IS_PLAYER) {
    // Intents relayed from other windows. Tauri 2's frontend `emit()` already
    // broadcasts cross-window natively, and the backend relays the same event
    // (excluding the sender) with `relayed: true` — process ONLY the relayed
    // copy, or every non-idempotent intent (add/remove/move) would run twice.
    // (`from === "main"` still guards our own echoes.)
    void listen<{ intent: QueueIntent; from: string; relayed?: boolean }>(
      "queue-intent",
      (ev) => {
        const p = ev.payload;
        if (!p || p.from === "main" || p.relayed !== true) return;
        try {
          applyQueueIntent(p.intent);
        } catch (e) {
          console.warn(`[navinamp] queue intent failed: ${e}`);
        }
      },
    );
    // A view (re)mounted and wants the current snapshot (relayed copies only —
    // same double-delivery guard as queue-intent above).
    void listen<{ relayed?: boolean }>("queue-state-request", (ev) => {
      if (ev.payload?.relayed !== true) return;
      broadcastQueueState();
    });
    // …and the current skin theme (the playlist window's pledit colors).
    void listen<{ relayed?: boolean }>("skin-theme-request", (ev) => {
      if (ev.payload?.relayed !== true) return;
      broadcastSkinTheme();
    });
    // Media keys / DEV transport / OS seek → the skin's action executor.
    void listen<{ command: MediaCommand; secs?: number }>("media-command", (ev) => {
      const c = ev.payload?.command;
      if (c === "play" || c === "pause" || c === "stop" || c === "next" || c === "prev") {
        mediaExecutor?.(c);
      } else if (c === "seekBy" || c === "seekTo") {
        const secs = ev.payload?.secs;
        if (typeof secs === "number" && Number.isFinite(secs)) mediaExecutor?.(c, secs);
      }
    });
    // Heal: another window may have been focused before our listeners
    // existed (it answers by re-broadcasting its current state).
    void emit("app-focus-request", {});
    // Broadcast after every queue mutation, coalesced: PLAY ALL can insert
    // tens of thousands of items and the effect deep-reads the whole queue,
    // so a burst of mutations must not serialize + broadcast it repeatedly.
    // 150ms is below perception; the snapshot of the latest run wins.
    // Wait for restoreQueue(): its async disk load completes after this
    // module graph initializes, and saving the empty state first would
    // overwrite the file we are about to restore.
    $effect(() => {
      if (!queueReady.value) return;
      const snapshot = readSnapshot();
      const timer = setTimeout(() => {
        void emit("queue-state", { from: "main", snapshot });
        persistQueue(snapshot);
      }, 150);
      return () => clearTimeout(timer);
    });
    // Best-effort flush if the player window closes inside the debounce.
    // `beforeunload` is not reliable in the Tauri webview; the close path is.
    window.addEventListener("beforeunload", () => {
      persistQueue(readSnapshot());
      persistPlayback();
    });
    void getCurrentWindow()
      .onCloseRequested(() => {
        persistQueue(readSnapshot());
        persistPlayback();
      })
      .catch(() => undefined);
    return;
  }

  // Non-player: consume the player's snapshots, ask for one on mount and
  // whenever this webview becomes visible again (heals missed snapshots —
  // e.g. the player was still booting when the view first asked). The skin
  // theme rides the same re-heal: ask once on mount / refocus.
  void listen<{ from: string; snapshot: QueueSnapshot }>("queue-state", (ev) => {
    const p = ev.payload;
    if (!p || p.from !== "main" || !p.snapshot) return;
    queueSnapshot.value = p.snapshot;
  });
  void listen<{ from: string; theme: SkinTheme }>("skin-theme", (ev) => {
    const p = ev.payload;
    if (!p || p.from !== "main" || !p.theme) return;
    skinTheme.value = p.theme;
  });
  void emit("queue-state-request", {});
  void emit("skin-theme-request", {});
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden) {
      void emit("queue-state-request", {});
      void emit("skin-theme-request", {});
    }
  });
}
