/**
 * Typed IPC surface for the 10-band equalizer backend (M6).
 *
 * Shapes mirror the frozen M6 contract 1:1 (`eq_state` / `eq_set_enabled` /
 * `eq_set_preamp` / `eq_set_band` / `eq_presets`); field names are camelCase
 * on the wire (the Rust side serializes `rename_all = "camelCase"`, same as
 * `audio_state`). Wrappers stay thin so callers decide the error policy:
 * the EQ panel fire-and-forgets slider drags and turns a missing command
 * into an inline note via `friendlyIpcError` (library/ipc). The dB↔fraction
 * helpers + `EqLive` below serve the skinned EQ window (M6c), whose skin
 * sliders drag in 0..1 space over the ±12 dB range.
 */

import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";

/** Fixed band count of the classic Winamp EQ. */
export const EQ_BAND_COUNT = 10;

export const EQ_DB_MIN = -12;
export const EQ_DB_MAX = 12;
/** Full-scale span in dB (skin sliders drag in 0..1 fraction space). */
export const EQ_DB_RANGE = EQ_DB_MAX - EQ_DB_MIN;

/** dB → 0..1 slider fraction (0 = -12 dB, 0.5 = 0 dB, 1 = +12 dB). */
export function dbToFraction(db: number): number {
  return (db - EQ_DB_MIN) / EQ_DB_RANGE;
}

/** 0..1 slider fraction → dB (inverse of `dbToFraction`). */
export function fractionToDb(fraction: number): number {
  return fraction * EQ_DB_RANGE + EQ_DB_MIN;
}

/** Snapshot returned by `eq_state`. */
export interface EqState {
  enabled: boolean;
  preampDb: number;
  bandsDb: number[];
}

/**
 * Live EQ view-state the skinned EQ window renders from (M6c): the polled
 * `eq_state` plus the renderer-local AUTO flag (there is no backend AUTO
 * concept yet — the toggle is visual only and not persisted).
 */
export interface EqLive {
  enabled: boolean;
  preampDb: number;
  bandsDb: number[];
  auto: boolean;
}

/** One EQ preset: a classic built-in curve or a user save. */
export interface EqPreset {
  name: string;
  bandsDb: number[];
  /** True for presets saved in the app (deletable). */
  user?: boolean;
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

/** Classic Winamp presets plus the user's saved curves. Preamp is not part of a preset. */
export function eqPresets(): Promise<EqPreset[]> {
  return invoke<EqPreset[]>("eq_presets");
}

/**
 * Save the current band curve under `name` (upsert). Returns the refreshed
 * preset list; built-in names are rejected by the backend.
 */
export function eqSavePreset(name: string, bandsDb: number[]): Promise<EqPreset[]> {
  return invoke<EqPreset[]>("eq_save_preset", { name, bandsDb });
}

/** Delete a user preset by name; returns the refreshed preset list. */
export function eqDeletePreset(name: string): Promise<EqPreset[]> {
  return invoke<EqPreset[]>("eq_delete_preset", { name });
}

/** Broadcast: the user preset list changed (another window saved/deleted). */
export const EQ_PRESETS_EVENT = "eq-presets-changed";

export async function notifyPresetsChanged(): Promise<void> {
  await emit(EQ_PRESETS_EVENT, {});
}

export async function onPresetsChanged(handler: () => void): Promise<() => void> {
  return listen(EQ_PRESETS_EVENT, () => handler());
}
