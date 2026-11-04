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
import type { Track } from "../library/ipc";

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
  /** Selected library (music folder) id; null = all accessible libraries. */
  libraryId: string | null;
  /** Active server profile id (stable across restarts), or null when none. */
  profileId: string | null;
  /** Active profile's display name (custom label, else `user@host`). */
  profileLabel: string | null;
}

/** One saved Navidrome server (all are aggregated; one is active/default). */
export interface NavidromeProfile {
  id: string;
  /** Effective display name (custom label or `user@host`). */
  name: string;
  /** User-editable label; null = derive from `user@host`. */
  label: string | null;
  url: string;
  user: string;
  active: boolean;
  /** Unix **seconds** of this server's last completed sync, if any. */
  syncedAt: number | null;
}

/** One music library (music folder) on the server. */
export interface MusicFolder {
  id: string;
  name: string;
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

/** Every saved server (creation order) with the active one flagged. */
export function navidromeProfiles(): Promise<NavidromeProfile[]> {
  return invoke<NavidromeProfile[]>("navidrome_profiles");
}

/**
 * Verify credentials (ping) and save them. Connecting to a known url+user
 * updates that profile; a new pair adds one. The profile becomes active;
 * other servers' synced rows stay untouched (aggregated library).
 */
export function navidromeConnect(
  url: string,
  user: string,
  password: string,
  label: string | null = null,
  insecureTls = false,
): Promise<NavidromeStatus> {
  return invoke<NavidromeStatus>("navidrome_connect", {
    url,
    user,
    password,
    label,
    insecureTls,
  });
}

/** Forget the stored credentials; returns the (now unconfigured) status. */
export function navidromeDisconnect(): Promise<NavidromeStatus> {
  return invoke<NavidromeStatus>("navidrome_disconnect");
}

/**
 * Make a saved server the active/default one. All servers stay synced and
 * visible; active drives playlists, publishing and the status card.
 */
export function navidromeSwitch(id: string): Promise<NavidromeStatus> {
  return invoke<NavidromeStatus>("navidrome_switch", { id });
}

/**
 * Remove one saved server: its synced rows, credentials and playlist
 * mappings. Other servers' rows stay visible.
 */
export function navidromeRemoveProfile(id: string): Promise<NavidromeStatus> {
  return invoke<NavidromeStatus>("navidrome_remove_profile", { id });
}

/** Rename a saved server; null/blank clears back to `user@host`. */
export function navidromeRenameProfile(
  id: string,
  label: string | null,
): Promise<NavidromeStatus> {
  return invoke<NavidromeStatus>("navidrome_rename_profile", { id, label });
}

/**
 * Sync one server (`serverId`) or every saved server (`null`/omitted) into
 * the unified `tracks` table. A quick sync (`full = false`) fetches only
 * albums whose cheap index fingerprint changed; a full sync re-fetches every
 * album. Resolves with the aggregate report; a second concurrent call throws
 * ("one sync at a time" — the running sync's `navidrome-progress` events
 * keep the UI live).
 */
export function navidromeSync(
  serverId: string | null = null,
  full = false,
): Promise<NavidromeSyncReport> {
  return invoke<NavidromeSyncReport>("navidrome_sync", { serverId, full });
}

/** Translate a remote track id into a streamable http(s) URL. */
export function navidromeStreamUrl(
  serverId: string | null | undefined,
  remoteId: string,
): Promise<string> {
  return invoke<string>("navidrome_stream_url", { serverId: serverId ?? null, remoteId });
}

/** Scrobble: `submission=false` = "now playing", `true` = counted listen. */
export function navidromeScrobble(
  serverId: string | null | undefined,
  remoteId: string,
  submission: boolean,
): Promise<void> {
  return invoke<void>("navidrome_scrobble", {
    serverId: serverId ?? null,
    remoteId,
    submission,
  });
}

/** Star / unstar a remote track on the server. */
export function navidromeStar(
  serverId: string | null | undefined,
  remoteId: string,
  starred: boolean,
): Promise<void> {
  return invoke<void>("navidrome_star", { serverId: serverId ?? null, remoteId, starred });
}

/** Set the server-side rating (0 = clear, 1–5 stars) for a remote track. */
export function navidromeRate(
  serverId: string | null | undefined,
  remoteId: string,
  rating: number,
): Promise<void> {
  return invoke<void>("navidrome_rate", { serverId: serverId ?? null, remoteId, rating });
}

/** One server playlist from `getPlaylists`. */
export interface NavidromePlaylist {
  id: string;
  name: string;
  songCount: number;
  /** Total playlist duration in seconds. */
  durationSecs: number;
}

/** All playlists visible to the connected user (browse-only). */
export function navidromePlaylists(
  serverId: string | null = null,
): Promise<NavidromePlaylist[]> {
  return invoke<NavidromePlaylist[]>("navidrome_playlists", { serverId });
}

/** Libraries (music folders) accessible to the connected user. */
export function navidromeLibraries(): Promise<MusicFolder[]> {
  return invoke<MusicFolder[]>("navidrome_libraries");
}

/**
 * Select one library for sync (`null` = all libraries). The backend
 * validates the id; callers should trigger a sync so the local remote rows
 * match the new selection (the sync prunes rows outside it).
 */
export function navidromeSetLibrary(libraryId: string | null): Promise<NavidromeStatus> {
  return invoke<NavidromeStatus>("navidrome_set_library", { libraryId });
}

/**
 * One playlist's tracks as EPHEMERAL library rows (negative ids, source
 * `navidrome`). They play like synced remote rows but are never written to
 * the local DB, so the view always reflects the server.
 */
export function navidromePlaylistTracks(
  serverId: string | null,
  playlistId: string,
): Promise<Track[]> {
  return invoke<Track[]>("navidrome_playlist_tracks", { serverId, playlistId });
}

/** Result of publishing one smart view as a server playlist. */
export interface NavidromePublishReport {
  playlistId: string;
  name: string;
  trackCount: number;
  /** True on first publish, false when the mapped playlist was updated. */
  created: boolean;
}

/**
 * Publish a smart view as a Navidrome playlist (`createPlaylist` on the
 * first publish, `updatePlaylist` afterwards). Only the view's remote rows
 * are included; its limit/sort apply.
 */
export function navidromePublishSmartView(
  id: number,
  serverId: string | null = null,
): Promise<NavidromePublishReport> {
  return invoke<NavidromePublishReport>("navidrome_publish_smart_view", { id, serverId });
}

/**
 * Publish an explicit set of Subsonic song ids under `[NaVinaMP] <name>`.
 * Creates the playlist or updates the one with that exact name.
 */
export function navidromePublishTracks(
  name: string,
  songIds: string[],
  serverId: string | null = null,
): Promise<NavidromePublishReport> {
  return invoke<NavidromePublishReport>("navidrome_publish_tracks", {
    name,
    songIds,
    serverId,
  });
}

/** Persist streaming prefs; `bitrateKbps` is only meaningful for transcodes. */
export function navidromeSetStreaming(
  format: StreamFormat,
  bitrateKbps: number | null,
): Promise<NavidromeStatus> {
  return invoke<NavidromeStatus>("navidrome_set_streaming", { format, bitrateKbps });
}

// ─── events ─────────────────────────────────────────────────────────────────

/** `navidrome-progress` payload while a sync runs (throttled server-side).
 *  Values are **tracks**; `total` is 0 when the pre-walk count failed. */
export interface NavidromeProgressPayload {
  scanned: number;
  total: number;
  /** Profile id currently being walked (multi-server sync). */
  server?: string;
  /** Display name of that profile. */
  label?: string;
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
