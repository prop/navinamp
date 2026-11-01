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

/**
 * The skin every window loads on its own (player + EQ windows render skins;
 * each webview pulls the payload over IPC). Honours the user's persisted
 * choice, then falls back to the first installed archive, then the built-in
 * fallback skin. Throws when no backend / broken archive — callers degrade
 * to the mock skin (src/lib/skin/mock).
 */
export async function loadDefaultSkin(): Promise<SkinPayload> {
  const skins = await listSkins();
  const wanted = selectedSkinId();
  const pick =
    skins.find((s) => s.id === wanted) ??
    skins.find((s) => s.id !== "fallback") ??
    skins[0];
  if (!pick) throw new Error("no skins installed");
  console.log(`[navinamp] skin: ${pick.fileName}`);
  return loadSkin(pick.id);
}
