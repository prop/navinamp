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

import { IS_PLAYER, LABEL, type WindowLabel } from "./identity";
import { queue } from "../player/queue.svelte";
import type { QueueItem, QueueItemMeta, RepeatMode } from "../player/queue.svelte";

// ── window-manager IPC (frozen M6b contract) ───────────────────────────────

/** One OS window's state, as reported by `windows_get` / `windows-changed`. */
export interface WinInfo {
  visible: boolean;
  layout: "normal" | "shade";
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

export function windowSetSize(label: WindowLabel, w: number, h: number): Promise<void> {
  return invoke<void>("window_set_size", { label, w, h });
}

// ── queue intents (the queue's public methods, serialized) ─────────────────

export type QueueIntent =
  | { type: "play"; index: number }
  | { type: "add"; paths: string[] }
  | { type: "addFromLibrary"; metas: QueueItemMeta[] }
  | { type: "addNext"; path: string; index?: number }
  | { type: "remove"; index: number }
  | { type: "move"; from: number; to: number }
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

export type MediaCommand = "play" | "pause" | "stop" | "next" | "prev";

/** Apply one intent to the local queue — only meaningful in the player. */
export function applyQueueIntent(intent: QueueIntent): void {
  switch (intent.type) {
    case "play":
      queue.play(intent.index);
      break;
    case "add":
      queue.add(intent.paths);
      break;
    case "addFromLibrary":
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
 * "Play now" from a library view: append the metas, then play the first
 * appended index (mirrors queue.addFromLibrary + queue.play(first)). In a
 * non-player window without a snapshot yet we only enqueue — playing index
 * 0 of a stale view would start the wrong track.
 */
export function queuePlayNow(metas: QueueItemMeta[]): void {
  if (metas.length === 0) return;
  if (!IS_PLAYER && queueSnapshot.value == null) {
    queueIntent({ type: "addFromLibrary", metas });
    return;
  }
  const at = IS_PLAYER ? queue.items.length : (queueSnapshot.value?.items.length ?? 0);
  queueIntent({ type: "addFromLibrary", metas });
  queueIntent({ type: "play", index: at });
}

// ── reactive stores ────────────────────────────────────────────────────────

/** Window-manager state, kept fresh by `windows-changed`. */
export const windowsStore = $state<{ byLabel: Record<string, WinInfo> }>({ byLabel: {} });

/**
 * Latest queue snapshot — meaningful only in NON-player windows (the player
 * renders its own queue and never consumes snapshots).
 */
export const queueSnapshot = $state<{ value: QueueSnapshot | null }>({ value: null });

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

/** The player window's current layout (defaults to "normal"). */
export function playerLayoutMode(): "normal" | "shade" {
  return winInfo("main")?.layout ?? "normal";
}

// ── media commands (media keys / DEV transport) ────────────────────────────

let mediaExecutor: ((command: MediaCommand) => void) | null = null;

/**
 * Player-side hook: how to run media commands. Wired to the SAME executor
 * as the skin's action buttons so PLAY/PAUSE keep their resume-vs-restart
 * semantics no matter where the command originated.
 */
export function setMediaCommandExecutor(fn: (command: MediaCommand) => void): void {
  mediaExecutor = fn;
}

/** Frontend→frontend transport command (the library's DEV block uses it). */
export function emitMediaCommand(command: MediaCommand): void {
  void emit("media-command", { command });
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
          console.warn(`[navamp] queue intent failed: ${e}`);
        }
      },
    );
    // A view (re)mounted and wants the current snapshot (relayed copies only —
    // same double-delivery guard as queue-intent above).
    void listen<{ relayed?: boolean }>("queue-state-request", (ev) => {
      if (ev.payload?.relayed !== true) return;
      broadcastQueueState();
    });
    // Media keys / DEV transport → the skin's action executor.
    void listen<{ command: MediaCommand }>("media-command", (ev) => {
      const c = ev.payload?.command;
      if (c === "play" || c === "pause" || c === "stop" || c === "next" || c === "prev") {
        mediaExecutor?.(c);
      }
    });
    // Broadcast after EVERY queue mutation: the effect deep-reads the queue
    // (item spreads, index, toggles), so any change re-runs it.
    $effect(() => {
      const snapshot = readSnapshot();
      void emit("queue-state", { from: "main", snapshot });
    });
    return;
  }

  // Non-player: consume the player's snapshots, ask for one on mount and
  // whenever this webview becomes visible again (heals missed snapshots —
  // e.g. the player was still booting when the view first asked).
  void listen<{ from: string; snapshot: QueueSnapshot }>("queue-state", (ev) => {
    const p = ev.payload;
    if (!p || p.from !== "main" || !p.snapshot) return;
    queueSnapshot.value = p.snapshot;
  });
  void emit("queue-state-request", {});
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden) void emit("queue-state-request", {});
  });
}
