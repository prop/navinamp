/**
 * Skin loading: the real `skin_load` Tauri command.
 *
 * Returns the `SkinPayload` ({ skin, images, warnings }) for an installed
 * skin id (see `skin_list`). Throws when the backend rejects the id or the
 * archive is unusable — callers decide whether to fall back to the built-in
 * mock skin (src/lib/skin/mock.ts).
 */

import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import type { SkinPayload } from "./types";

/** localStorage key holding the selected skin id (absent = loader default). */
const SKIN_KEY = "navinamp.skin";

/** Frontend broadcast: a different skin was selected (`{ id }` payload). */
export const SKIN_CHANGED_EVENT = "skin-changed";

/** One installable skin, as returned by the `skin_list` command. */
export interface SkinInfo {
  id: string;
  fileName: string;
}

/** List the skins installed in the app's skins directory. */
export async function listSkins(): Promise<SkinInfo[]> {
  return invoke<SkinInfo[]>("skin_list");
}

/**
 * The curated "popular skins" pack (20 classic skins from the Winamp Skin
 * Museum's archive.org collection). Archives are downloaded on request — the
 * app ships only this manifest.
 */
export interface PopularSkin {
  id: string;
  name: string;
  author: string | null;
  fileName: string;
  url: string;
  size: number;
  sha256: string;
  /** License stated by the source item. */
  license: string;
  sourceUrl: string;
  /** Bytes on disk already match the manifest checksum. */
  installed: boolean;
}

export interface PopularPack {
  version: number;
  curatedAt: string;
  source: string;
  note: string;
  licenseNote: string;
  skins: PopularSkin[];
}

export interface PopularInstallReport {
  installed: number;
  skipped: number;
  failed: { id: string; error: string }[];
}

/** Progress event streamed by `skin_install_popular`. */
export const POPULAR_PROGRESS_EVENT = "popular-skins-progress";

export interface PopularProgress {
  done: number;
  total: number;
  id: string;
  name: string;
  error: string | null;
}

/** The curated pack + which entries are already installed. */
export function popularSkins(): Promise<PopularPack> {
  return invoke<PopularPack>("popular_skins");
}

/** Download missing pack skins into the app's skins directory (user action). */
export function installPopularSkins(): Promise<PopularInstallReport> {
  return invoke<PopularInstallReport>("skin_install_popular");
}

/** The persisted skin choice, or null when the loader should pick. */
export function selectedSkinId(): string | null {
  try {
    return localStorage.getItem(SKIN_KEY);
  } catch {
    return null; // storage unavailable: in-memory default for this session
  }
}

/**
 * Persist a skin choice and broadcast it to every window. The player and
 * EQ windows reload their skin payload on receipt (the library window does
 * not render skins). Callers should `loadSkin(id)` first to validate.
 */
export async function chooseSkin(id: string): Promise<void> {
  try {
    localStorage.setItem(SKIN_KEY, id);
  } catch {
    // Private/storage-less webview: the broadcast still applies it live.
  }
  await emit(SKIN_CHANGED_EVENT, { id });
}

/** Subscribe to skin changes; resolves an unlisten function. */
export async function onSkinChanged(handler: (id: string) => void): Promise<() => void> {
  return listen<{ id: string }>(SKIN_CHANGED_EVENT, (ev) => {
    const id = ev.payload?.id;
    if (typeof id === "string" && id.length > 0) handler(id);
  });
}

/** Load an installed skin by id (throws on unknown id / bad archive). */
export async function loadSkin(id: string): Promise<SkinPayload> {
  if (id.length === 0) throw new Error("skin id must not be empty");
  return invoke<SkinPayload>("skin_load", { id });
}

/** Ids served from bytes embedded in the binary, never from the skins dir. */
const BUILT_IN_IDS = new Set(["fallback", "base-2-91"]);

/**
 * The skin every window loads on its own (player + EQ windows render skins;
 * each webview pulls the payload over IPC). Honours the user's persisted
 * choice, then falls back to the first installed archive, then the bundled
 * Winamp 2.91 base skin, then the built-in fallback skin. Throws when no
 * backend / broken archive — callers degrade to the mock skin
 * (src/lib/skin/mock).
 */
export async function loadDefaultSkin(): Promise<SkinPayload> {
  const skins = await listSkins();
  const wanted = selectedSkinId();
  const pick =
    skins.find((s) => s.id === wanted) ??
    skins.find((s) => !BUILT_IN_IDS.has(s.id)) ??
    skins.find((s) => s.id === "base-2-91") ??
    skins[0];
  if (!pick) throw new Error("no skins installed");
  console.log(`[navinamp] skin: ${pick.fileName}`);
  return loadSkin(pick.id);
}
