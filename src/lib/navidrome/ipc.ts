/**
 * Typed IPC surface for the Navidrome (Subsonic / OpenSubsonic) backend (M5).
 *
 * Shapes mirror the frozen M5 contract 1:1 (`navidrome_status` /
 * `navidrome_connect` / … / `navidrome_set_streaming`, plus the
 * `navidrome-progress` / `navidrome-synced` events). Wrappers stay thin so
 * components `try/catch` around a single named command — while the Rust
 * side lands in parallel, a missing command rejects with a string that the
 * existing `friendlyIpcError` (library/ipc) renders as an inline
 * "not available in this build" note instead of a crash.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** `source` value of synced Navidrome rows in the unified library. */
export const REMOTE_SOURCE = "navidrome";

export type StreamFormat = "original" | "mp3" | "opus";

export const STREAM_FORMATS: StreamFormat[] = ["original", "mp3", "opus"];

/** User-selected transcode prefs (original → no bitrate). */
export interface NavidromeStreaming {
  format: StreamFormat;
  bitrateKbps: number | null;
}

/** Snapshot returned by status/connect/disconnect/set_streaming. */
export interface NavidromeStatus {
  configured: boolean;
  url: string | null;
  user: string | null;
  streaming: NavidromeStreaming;
  /** Unix **seconds** of the last completed sync, if any. */
  syncedAt: number | null;
  serverVersion: string | null;
  trackCount: number;
}

/** Everything the caller needs to know about one completed sync. */
export interface NavidromeSyncReport {
  artists: number;
  albums: number;
  tracksAdded: number;
  tracksUpdated: number;
  tracksRemoved: number;
  errors: string[];
  durationMs: number;
}

// ─── command wrappers ───────────────────────────────────────────────────────
// Tauri v2 maps camelCase JS argument keys onto the snake_case Rust params.

/** Current connection + streaming + sync snapshot. */
export function navidromeStatus(): Promise<NavidromeStatus> {
  return invoke<NavidromeStatus>("navidrome_status");
}

/** Verify credentials (ping) and persist them; throws on auth failure. */
export function navidromeConnect(
  url: string,
  user: string,
  password: string,
): Promise<NavidromeStatus> {
  return invoke<NavidromeStatus>("navidrome_connect", { url, user, password });
}

/** Forget the stored credentials; returns the (now unconfigured) status. */
export function navidromeDisconnect(): Promise<NavidromeStatus> {
  return invoke<NavidromeStatus>("navidrome_disconnect");
}

/**
 * Full library sync into the unified `tracks` table. Resolves with the
 * report; a second concurrent call throws ("one sync at a time" — the
 * running sync's `navidrome-progress` events keep the UI live).
 */
export function navidromeSync(): Promise<NavidromeSyncReport> {
  return invoke<NavidromeSyncReport>("navidrome_sync");
}

/** Translate a remote track id into a streamable http(s) URL. */
export function navidromeStreamUrl(remoteId: string): Promise<string> {
  return invoke<string>("navidrome_stream_url", { remoteId });
}

/** Scrobble: `submission=false` = "now playing", `true` = counted listen. */
export function navidromeScrobble(remoteId: string, submission: boolean): Promise<void> {
  return invoke<void>("navidrome_scrobble", { remoteId, submission });
}

/** Star / unstar a remote track on the server. */
export function navidromeStar(remoteId: string, starred: boolean): Promise<void> {
  return invoke<void>("navidrome_star", { remoteId, starred });
}

/** Persist streaming prefs; `bitrateKbps` is only meaningful for transcodes. */
export function navidromeSetStreaming(
  format: StreamFormat,
  bitrateKbps: number | null,
): Promise<NavidromeStatus> {
  return invoke<NavidromeStatus>("navidrome_set_streaming", { format, bitrateKbps });
}

// ─── events ─────────────────────────────────────────────────────────────────

/** `navidrome-progress` payload while a sync runs (throttled server-side). */
export interface NavidromeProgressPayload {
  scanned: number;
  total: number;
}

/** `navidrome-synced` payload: the final report, or `{ error }` on failure. */
export type NavidromeSyncedPayload = NavidromeSyncReport | { error: string };

export function isSyncReport(p: NavidromeSyncedPayload): p is NavidromeSyncReport {
  return (p as NavidromeSyncReport).tracksAdded !== undefined;
}

/** Subscribe to `navidrome-progress`; resolves an unlisten function. */
export function onNavidromeProgress(
  handler: (p: NavidromeProgressPayload) => void,
): Promise<() => void> {
  return listen<NavidromeProgressPayload>("navidrome-progress", (ev) => handler(ev.payload));
}

/** Subscribe to `navidrome-synced`; resolves an unlisten function. */
export function onNavidromeSynced(
  handler: (p: NavidromeSyncedPayload) => void,
): Promise<() => void> {
  return listen<NavidromeSyncedPayload>("navidrome-synced", (ev) => handler(ev.payload));
}
