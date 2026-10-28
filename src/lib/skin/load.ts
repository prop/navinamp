/**
 * Skin loading: the real `skin_load` Tauri command.
 *
 * Returns the `SkinPayload` ({ skin, images, warnings }) for an installed
 * skin id (see `skin_list`). Throws when the backend rejects the id or the
 * archive is unusable — callers decide whether to fall back to the built-in
 * mock skin (src/lib/skin/mock.ts).
 */

import { invoke } from "@tauri-apps/api/core";
import type { SkinPayload } from "./types";

/** One installable skin, as returned by the `skin_list` command. */
export interface SkinInfo {
  id: string;
  fileName: string;
}

/** List the skins installed in the app's skins directory. */
export async function listSkins(): Promise<SkinInfo[]> {
  return invoke<SkinInfo[]>("skin_list");
}

/** Load an installed skin by id (throws on unknown id / bad archive). */
export async function loadSkin(id: string): Promise<SkinPayload> {
  if (id.length === 0) throw new Error("skin id must not be empty");
  return invoke<SkinPayload>("skin_load", { id });
}
