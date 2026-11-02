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
}

interface AssocStore {
  tracks: Record<string, EqTrackOverride>;
  artists: Record<string, number[]>;
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
        };
      }
    }
  }
  if (source.artists != null && typeof source.artists === "object") {
    for (const [key, value] of Object.entries(source.artists)) {
      const bands = normalizeBands(value);
      if (bands != null) store.artists[key] = bands;
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

/** The saved curve for a track path, if any. */
export function getTrackEq(path: string): EqTrackOverride | null {
  return readStore().tracks[path] ?? null;
}

/** The saved curve for an artist (case-insensitive), if any. */
export function getArtistEq(artist: string): number[] | null {
  const key = artistKey(artist);
  return key === "" ? null : (readStore().artists[key] ?? null);
}

/** Capture the live EQ bands for a track or an artist. */
export async function rememberEq(
  path: string,
  artist: string,
  scope: EqAssocScope,
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
    store.tracks[path] = { bandsDb: bands, artist: resolvedArtist };
  } else {
    // The artist curve should apply to this track now, so the narrower
    // track override must go.
    delete store.tracks[path];
    store.artists[artistKey(resolvedArtist)] = bands;
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
