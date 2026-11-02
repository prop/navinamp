/**
 * EQ AUTO associations: per-track / per-artist curves saved from the library
 * and playlist context menus.
 *
 * Resolution order in the EQ window: track curve → artist curve → preset
 * named after the track's genre. Stored in localStorage (shared by every app
 * window, like the skin pick) and broadcast on save/clear so the currently
 * playing track updates immediately.
 *
 * A track override is a capture of the live EQ bands at save time, not a
 * preset name: tweak the curve to taste, then "save for this track". Saving
 * an artist curve drops that track's override so the artist curve applies.
 *
 * The EQ panel's AUTO CURVES manager lists and deletes these associations;
 * removals broadcast the same change event so the playing track falls back
 * through the resolution order immediately.
 */

import { emit, listen } from "@tauri-apps/api/event";
import { EQ_BAND_COUNT, EQ_DB_MAX, EQ_DB_MIN, eqState } from "./eq";
import { libraryTrack } from "../library/ipc";

const STORAGE_KEY = "navinamp.eq.assoc";

/** Broadcast after associations change (all windows). */
export const EQ_ASSOC_EVENT = "eq-assoc-changed";

export type EqAssocScope = "track" | "artist";

/** Broadcast payload: which row/artist a saved or cleared curve affects. */
export interface EqAssocChange {
  path: string;
  artist: string;
  scope: EqAssocScope | "clear";
}

export interface EqTrackOverride {
  /** Ten band gains in dB. */
  bandsDb: number[];
  /** Artist captured with the override (display/diagnostics only). */
  artist: string;
  /** Track title captured at save time; absent on older entries. */
  title?: string;
}

export interface EqArtistOverride {
  /** Ten band gains in dB. */
  bandsDb: number[];
  /** Display name captured at save time (the store key is lowercased). */
  artist: string;
}

/** One saved track curve, as shown by the AUTO CURVES manager. */
export interface EqTrackEntry {
  path: string;
  artist: string;
  /** Captured title, or "" for entries saved before titles were stored. */
  title: string;
  bandsDb: number[];
}

/** One saved artist curve, as shown by the AUTO CURVES manager. */
export interface EqArtistEntry {
  /** Lowercased store key (pass back to `removeEqArtist`). */
  key: string;
  artist: string;
  bandsDb: number[];
}

export interface EqAssociationList {
  tracks: EqTrackEntry[];
  artists: EqArtistEntry[];
}

interface AssocStore {
  tracks: Record<string, EqTrackOverride>;
  artists: Record<string, EqArtistOverride>;
}

/** Fallback for webviews where localStorage is unavailable (session only). */
let memory: AssocStore = { tracks: {}, artists: {} };

function emptyStore(): AssocStore {
  return { tracks: {}, artists: {} };
}

function clampDb(value: number): number {
  return Math.max(EQ_DB_MIN, Math.min(EQ_DB_MAX, value));
}

/** Accept only a well-formed 10-band array; clamp/zero anything odd. */
function normalizeBands(value: unknown): number[] | null {
  if (!Array.isArray(value) || value.length !== EQ_BAND_COUNT) return null;
  return value.map((v) =>
    typeof v === "number" && Number.isFinite(v) ? clampDb(v) : 0,
  );
}

function artistKey(artist: string): string {
  return artist.trim().toLowerCase();
}

function safeStorage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null; // storage-less webview
  }
}

function parseStore(raw: string): AssocStore {
  const store = emptyStore();
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return store;
  }
  if (parsed == null || typeof parsed !== "object") return store;
  const source = parsed as Partial<AssocStore>;
  if (source.tracks != null && typeof source.tracks === "object") {
    for (const [path, value] of Object.entries(source.tracks)) {
      const entry = value as Partial<EqTrackOverride> | null;
      const bands = normalizeBands(entry?.bandsDb);
      if (bands != null) {
        store.tracks[path] = {
          bandsDb: bands,
          artist: typeof entry?.artist === "string" ? entry.artist : "",
          title: typeof entry?.title === "string" ? entry.title : "",
        };
      }
    }
  }
  if (source.artists != null && typeof source.artists === "object") {
    for (const [key, value] of Object.entries(source.artists)) {
      // v1 stored a bare band array; newer entries carry the display name.
      const legacyBands = normalizeBands(value);
      if (legacyBands != null) {
        store.artists[key] = { bandsDb: legacyBands, artist: key };
        continue;
      }
      const entry = value as Partial<EqArtistOverride> | null;
      const bands = normalizeBands(entry?.bandsDb);
      if (bands != null) {
        const name =
          typeof entry?.artist === "string" && entry.artist.trim() !== ""
            ? entry.artist.trim()
            : key;
        store.artists[key] = { bandsDb: bands, artist: name };
      }
    }
  }
  return store;
}

function readStore(): AssocStore {
  const storage = safeStorage();
  if (storage == null) return memory;
  try {
    const raw = storage.getItem(STORAGE_KEY);
    return raw == null ? emptyStore() : parseStore(raw);
  } catch {
    return memory;
  }
}

function writeStore(store: AssocStore): void {
  memory = store;
  const storage = safeStorage();
  if (storage == null) return;
  try {
    storage.setItem(STORAGE_KEY, JSON.stringify(store));
  } catch {
    // quota/denied: the session-only copy still works in this window
  }
}

/**
 * Pre-aggregation remote paths were `navidrome://<remote_id>` with no server
 * segment. Return that legacy key for a server-qualified path so curves saved
 * before the migration still match.
 */
function legacyRemotePath(path: string): string | null {
  const prefix = "navidrome://";
  if (!path.startsWith(prefix)) return null;
  const rest = path.slice(prefix.length);
  const slash = rest.indexOf("/");
  if (slash < 0) return null;
  const remote = rest.slice(slash + 1);
  return remote === "" ? null : `${prefix}${remote}`;
}

/** The saved curve for a track path, if any. */
export function getTrackEq(path: string): EqTrackOverride | null {
  const store = readStore();
  const direct = store.tracks[path];
  if (direct != null) return direct;
  const legacy = legacyRemotePath(path);
  return legacy == null ? null : (store.tracks[legacy] ?? null);
}

/** The saved curve for an artist (case-insensitive), if any. */
export function getArtistEq(artist: string): number[] | null {
  const key = artistKey(artist);
  return key === "" ? null : (readStore().artists[key]?.bandsDb ?? null);
}

/** Capture the live EQ bands for a track or an artist. */
export async function rememberEq(
  path: string,
  artist: string,
  scope: EqAssocScope,
  title = "",
): Promise<void> {
  let resolvedArtist = artist.trim();
  if (scope === "artist" && resolvedArtist === "") {
    try {
      resolvedArtist = (await libraryTrack(path))?.artist.trim() ?? "";
    } catch {
      // unsynced/unknown track: no artist to key on
    }
  }
  if (scope === "artist" && resolvedArtist === "") {
    throw new Error("no artist tag to save under");
  }

  const live = await eqState();
  const bands =
    normalizeBands(live.bandsDb) ?? Array<number>(EQ_BAND_COUNT).fill(0);
  const store = readStore();
  if (scope === "track") {
    store.tracks[path] = {
      bandsDb: bands,
      artist: resolvedArtist,
      title: title.trim(),
    };
  } else {
    // The artist curve should apply to this track now, so the narrower
    // track override must go (including the legacy-path copy).
    delete store.tracks[path];
    const legacy = legacyRemotePath(path);
    if (legacy != null) delete store.tracks[legacy];
    store.artists[artistKey(resolvedArtist)] = {
      bandsDb: bands,
      artist: resolvedArtist,
    };
  }
  writeStore(store);
  await emit<EqAssocChange>(EQ_ASSOC_EVENT, {
    path,
    artist: resolvedArtist,
    scope,
  });
}

/**
 * Remove the track and artist curves for this row. Returns whether anything
 * changed. Resolves the artist through the library when the row does not
 * carry one (queue entries added by path).
 */
export async function clearEqFor(
  path: string,
  artist: string,
): Promise<boolean> {
  let resolvedArtist = artist.trim();
  if (resolvedArtist === "") {
    try {
      resolvedArtist = (await libraryTrack(path))?.artist.trim() ?? "";
    } catch {
      // no library row: only the track override can be cleared
    }
  }
  const store = readStore();
  let changed = false;
  if (store.tracks[path] != null) {
    delete store.tracks[path];
    changed = true;
  }
  const legacy = legacyRemotePath(path);
  if (legacy != null && store.tracks[legacy] != null) {
    delete store.tracks[legacy];
    changed = true;
  }
  const key = artistKey(resolvedArtist);
  if (key !== "" && store.artists[key] != null) {
    delete store.artists[key];
    changed = true;
  }
  if (changed) {
    writeStore(store);
    await emit<EqAssocChange>(EQ_ASSOC_EVENT, {
      path,
      artist: resolvedArtist,
      scope: "clear",
    });
  }
  return changed;
}

/** Subscribe to association changes (resolves an unlisten function). */
export function onEqAssocChanged(
  handler: (change: EqAssocChange) => void,
): Promise<() => void> {
  return listen<EqAssocChange>(EQ_ASSOC_EVENT, (ev) => handler(ev.payload));
}

/** Snapshot of every saved association for the AUTO CURVES manager. */
export function listEqAssociations(): EqAssociationList {
  const store = readStore();
  const collator = new Intl.Collator(undefined, {
    sensitivity: "base",
    numeric: true,
  });
  const tracks = Object.entries(store.tracks).map(
    ([path, entry]): EqTrackEntry => ({
      path,
      artist: entry.artist,
      title: entry.title ?? "",
      bandsDb: [...entry.bandsDb],
    }),
  );
  tracks.sort(
    (a, b) =>
      collator.compare(a.artist || a.path, b.artist || b.path) ||
      collator.compare(a.title || a.path, b.title || b.path),
  );
  const artists = Object.entries(store.artists)
    .map(
      ([key, entry]): EqArtistEntry => ({
        key,
        artist: entry.artist,
        bandsDb: [...entry.bandsDb],
      }),
    )
    .sort((a, b) => collator.compare(a.artist, b.artist));
  return { tracks, artists };
}

/**
 * Delete one saved track curve. Emits the usual clear event so the EQ window
 * re-evaluates the current track (falling back to the artist curve).
 */
export async function removeEqTrack(path: string): Promise<boolean> {
  const store = readStore();
  const entry = store.tracks[path];
  if (entry == null) return false;
  delete store.tracks[path];
  writeStore(store);
  await emit<EqAssocChange>(EQ_ASSOC_EVENT, {
    path,
    artist: entry.artist,
    scope: "clear",
  });
  return true;
}

/** Delete one saved artist curve by its store key (see `listEqAssociations`). */
export async function removeEqArtist(key: string): Promise<boolean> {
  const store = readStore();
  const entry = store.artists[key];
  if (entry == null) return false;
  delete store.artists[key];
  writeStore(store);
  await emit<EqAssocChange>(EQ_ASSOC_EVENT, {
    path: "",
    artist: entry.artist,
    scope: "clear",
  });
  return true;
}
