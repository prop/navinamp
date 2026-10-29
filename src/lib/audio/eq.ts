/**
 * Typed IPC surface for the 10-band equalizer backend (M6).
 *
 * Shapes mirror the frozen M6 contract 1:1 (`eq_state` / `eq_set_enabled` /
 * `eq_set_preamp` / `eq_set_band` / `eq_presets`); field names are camelCase
 * on the wire (the Rust side serializes `rename_all = "camelCase"`, same as
 * `audio_state`). Wrappers stay thin so callers decide the error policy:
 * the EQ panel fire-and-forgets slider drags and turns a missing command
 * into an inline note via `friendlyIpcError` (library/ipc).
 */

import { invoke } from "@tauri-apps/api/core";

/** Fixed band count of the classic Winamp EQ. */
export const EQ_BAND_COUNT = 10;

export const EQ_DB_MIN = -12;
export const EQ_DB_MAX = 12;

/** Snapshot returned by `eq_state`. */
export interface EqState {
  enabled: boolean;
  preampDb: number;
  bandsDb: number[];
}

/** One classic Winamp EQ preset, as returned by `eq_presets`. */
export interface EqPreset {
  name: string;
  bandsDb: number[];
}

/** Current EQ state (10 bands; bootstrap source for the panel). */
export function eqState(): Promise<EqState> {
  return invoke<EqState>("eq_state");
}

/** Enable / bypass the EQ chain. */
export function eqSetEnabled(enabled: boolean): Promise<void> {
  return invoke<void>("eq_set_enabled", { enabled });
}

/** Set the preamp gain in dB (±12). */
export function eqSetPreamp(db: number): Promise<void> {
  return invoke<void>("eq_set_preamp", { db });
}

/** Set one band (0..9) gain in dB (±12). */
export function eqSetBand(index: number, db: number): Promise<void> {
  return invoke<void>("eq_set_band", { index, db });
}

/** Classic Winamp presets ("Rock", "Pop", …). Preamp is deliberately not part of a preset. */
export function eqPresets(): Promise<EqPreset[]> {
  return invoke<EqPreset[]>("eq_presets");
}
