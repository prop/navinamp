/**
 * Typed IPC surface for the library + smart-view backend (M3/M4).
 *
 * Every shape mirrors the Rust side exactly (`crates/navamp-library/src/model.rs`
 * serializes `camelCase`; the smart-view contract is mirrored 1:1 from the
 * frozen M4 spec). Wrappers are thin so components can `try/catch` around a
 * single named command — a command the running backend does not implement yet
 * (smart views are landing in parallel) surfaces as a string error that
 * `friendlyIpcError` turns into an inline "not available in this build" note
 * instead of a crash.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// ─── library_* payload types (mirrors of the Rust structs) ─────────────────

/** One row of the `tracks` table, as returned by every query. */
export interface Track {
  id: number;
  /** Absolute path as stored in the DB (lossy string). */
  path: string;
  /** File mtime in Unix **milliseconds**. */
  mtime: number;
  size: number;
  title: string;
  artist: string;
  albumArtist: string;
  album: string;
  /** Grouping key `(album_artist || artist)\x00album` (normalized). */
  albumKey: string;
  genre: string;
  year: number | null;
  trackNo: number | null;
  discNo: number | null;
  durationSecs: number;
  bitrateKbps: number | null;
  samplerate: number | null;
  channels: number | null;
  /** Short container/codec id: `mp3`, `flac`, `ogg`, `opus`, `m4a`, … */
  codec: string;
  rating: number;
  playCount: number;
  /** Unix seconds; `null` = never played. */
  lastPlayedAt: number | null;
  /** Unix seconds, set when the track was first inserted. */
  dateAdded: number;
  /** `local` today; `navidrome` rows arrive with the M5 sync. */
  source: string;
  remoteId: string | null;
  /** Reserved for the album-art pipeline; always `null` for now. */
  artId: string | null;
}

/** An album as a grouped view over `tracks.albumKey`. */
export interface Album {
  albumKey: string;
  /** Display name (one representative of the group). */
  name: string;
  /** Album artist, falling back to the track artist. */
  artist: string;
  /** Earliest year found among the album's tracks, if any. */
  year: number | null;
  trackCount: number;
  totalSecs: number;
}

/** Whole-library counters for the status bar / empty-state UI. */
export interface LibraryStats {
  tracks: number;
  albums: number;
  artists: number;
  totalSecs: number;
  /** Unix **milliseconds** of the last completed scan, if any. */
  lastScan: number | null;
}

/** Why a single file could not be indexed (never fatal). */
export interface ScanFileError {
  path: string;
  error: string;
}

/** Everything the caller needs to know about one completed scan. */
export interface ScanReport {
  /** Canonical scan roots actually used. */
  roots: string[];
  filesFound: number;
  added: number;
  updated: number;
  unchanged: number;
  /** Rows removed because their files vanished under the scanned roots. */
  removed: number;
  errors: ScanFileError[];
  durationMs: number;
}

// ─── smart-view types (frozen M4 contract — do not deviate) ────────────────

export type SmartField =
  // string fields
  | "title"
  | "artist"
  | "album"
  | "genre"
  | "path"
  | "source"
  // number fields
  | "year"
  | "rating"
  | "playCount"
  | "durationSecs"
  | "bitrateKbps"
  // date fields
  | "lastPlayed"
  | "dateAdded";

export type StringOp = "is" | "isNot" | "contains" | "notContains" | "startsWith" | "endsWith";
export type NumberOp = "eq" | "ne" | "gt" | "gte" | "lt" | "lte" | "between";
export type DateOp = "inLastDays" | "notInLastDays" | "before" | "after";
export type SmartOp = StringOp | NumberOp | DateOp;

export type SmartSortField =
  | "title"
  | "artist"
  | "album"
  | "random"
  | "mostPlayed"
  | "recentlyAdded"
  | "recentlyPlayed";

export type SmartValue = string | number;

export interface RuleRow {
  field: SmartField;
  op: SmartOp;
  value: SmartValue;
  value2?: SmartValue;
}

export interface RuleGroup {
  match: "all" | "any";
  rules: RuleRow[];
}

export interface SmartViewDef {
  name: string;
  group: RuleGroup;
  limit: number | null;
  sort: SmartSortField;
  sortDesc: boolean;
}

export interface SavedSmartView {
  id: number;
  def: SmartViewDef;
}

// ─── command wrappers ───────────────────────────────────────────────────────
// Tauri v2 maps camelCase JS argument keys onto the snake_case Rust params.

export interface TrackQuery {
  artist?: string;
  genre?: string;
  /** Filter by library source, e.g. `local` / `navidrome` (M5). */
  source?: string;
  limit?: number;
  offset?: number;
}

/** Scan `roots` incrementally in the background; resolves with the report. */
export function libraryScan(roots: string[]): Promise<ScanReport> {
  return invoke<ScanReport>("library_scan", { roots });
}

/** Ranked FTS5 search over title/artist/album/genre. */
export function librarySearch(q: string, limit?: number): Promise<Track[]> {
  return invoke<Track[]>("library_search", { q, limit });
}

/** All albums grouped by album key, ordered artist → year → title. */
export function libraryAlbums(): Promise<Album[]> {
  return invoke<Album[]>("library_albums");
}

/** One album's tracks in disc/track order. */
export function libraryAlbumTracks(albumKey: string): Promise<Track[]> {
  return invoke<Track[]>("library_album_tracks", { albumKey });
}

/** Tracks filtered by optional artist/genre with limit/offset paging. */
export function libraryTracks(query: TrackQuery = {}): Promise<Track[]> {
  return invoke<Track[]>("library_tracks", query);
}

/** Library counters (`lastScan` is null until the first scan). */
export function libraryStats(): Promise<LibraryStats> {
  return invoke<LibraryStats>("library_stats");
}

/** The persisted scan roots, as configured by the last `library_scan`. */
export function libraryRoots(): Promise<string[]> {
  return invoke<string[]>("library_roots");
}

export function smartViewsList(): Promise<SavedSmartView[]> {
  return invoke<SavedSmartView[]>("smart_views_list");
}

/** Persist `def`; `id === null` creates (the new id is returned). */
export function smartViewSave(id: number | null, def: SmartViewDef): Promise<number> {
  return invoke<number>("smart_view_save", { id, def });
}

export function smartViewDelete(id: number): Promise<void> {
  return invoke<void>("smart_view_delete", { id });
}

export function smartViewRun(id: number): Promise<Track[]> {
  return invoke<Track[]>("smart_view_run", { id });
}

/** Evaluate an unsaved def (live preview in the rule builder). */
export function smartViewPreview(def: SmartViewDef): Promise<Track[]> {
  return invoke<Track[]>("smart_view_preview", { def });
}

// ─── events ─────────────────────────────────────────────────────────────────

/** `library-progress` payload while a scan runs. */
export interface ScanProgressPayload {
  scanned: number;
  total: number;
}

/** `library-scanned` payload: the final report, or `{ error }` on failure. */
export type LibraryScannedPayload = ScanReport | { error: string };

export function isScanReport(p: LibraryScannedPayload): p is ScanReport {
  return (p as ScanReport).roots !== undefined;
}

/** Subscribe to `library-progress`; resolves an unlisten function. */
export function onLibraryProgress(handler: (p: ScanProgressPayload) => void): Promise<() => void> {
  return listen<ScanProgressPayload>("library-progress", (ev) => handler(ev.payload));
}

/** Subscribe to `library-scanned`; resolves an unlisten function. */
export function onLibraryScanned(
  handler: (p: LibraryScannedPayload) => void,
): Promise<() => void> {
  return listen<LibraryScannedPayload>("library-scanned", (ev) => handler(ev.payload));
}

// ─── error friendliness ─────────────────────────────────────────────────────

/**
 * Turn an invoke rejection into a user-facing line. Commands that the
 * running backend does not register yet (the smart-view pair is landing in
 * parallel with this UI) are reported as "not available" instead of the raw
 * Tauri "Command X not found" noise.
 */
export function friendlyIpcError(what: string, e: unknown): string {
  const msg = e instanceof Error ? e.message : String(e);
  if (/command .+ not found|unknown command|not registered|not available/i.test(msg)) {
    return `${what} is not available in this build yet (backend command missing)`;
  }
  return `${what}: ${msg}`;
}
