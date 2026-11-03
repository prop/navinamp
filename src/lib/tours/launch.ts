/**
 * Cross-window tour launch.
 *
 * The player window owns the first-run / What's-new prompts but the tour
 * renders in the library window. A pending key bridges the case where the
 * library is still mounting (the Tauri event would be missed); the event
 * handles the already-open case.
 */

import { emit } from "@tauri-apps/api/event";
import { windowSetVisible } from "../window/bus.svelte";

export const TOUR_EVENT = "tour-launch";
const PENDING_KEY = "navinamp.tours.launch";

export interface TourLaunch {
  id: string;
  index?: number;
}

/** Open the library and start a tour there (player/what's-new entry point). */
export function launchTour(id: string, index = 0): void {
  const payload: TourLaunch = { id, index };
  try {
    localStorage.setItem(PENDING_KEY, JSON.stringify(payload));
  } catch {
    // storage-less webview: the event still covers an already-open library
  }
  void windowSetVisible("library", true).catch(() => undefined);
  void emit(TOUR_EVENT, payload);
}

/** Start a tour from inside the library (replay button). */
export function startTourHere(id: string, index = 0): void {
  void emit(TOUR_EVENT, { id, index } satisfies TourLaunch);
}

/** Consume a launch left by the player while the library was mounting. */
export function takePendingLaunch(): TourLaunch | null {
  try {
    const raw = localStorage.getItem(PENDING_KEY);
    if (raw == null) return null;
    localStorage.removeItem(PENDING_KEY);
    const parsed = JSON.parse(raw) as Partial<TourLaunch> | null;
    if (typeof parsed?.id !== "string") return null;
    return {
      id: parsed.id,
      index: typeof parsed.index === "number" ? parsed.index : 0,
    };
  } catch {
    return null;
  }
}

/** Clear a pending launch once an event-driven start consumes it. */
export function clearPendingLaunch(): void {
  try {
    localStorage.removeItem(PENDING_KEY);
  } catch {
    // nothing to clear
  }
}
