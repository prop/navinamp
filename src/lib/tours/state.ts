/**
 * Persistent tour progress.
 *
 * Stored in localStorage (already shared by this app's windows, like the skin
 * choice). The player window marks the app version as seen when it offers the
 * first-run card or the What's-new pill; completed/dismissed tours never
 * reappear unless replayed manually.
 */

import { TOURS, type Tour } from "./registry";

const KEY = "navinamp.tours.v1";

export interface TourProgress {
  completed: Record<string, boolean>;
  dismissed: Record<string, boolean>;
  /** Last app version for which onboarding was offered. "" = first run. */
  seenVersion: string;
}

function empty(): TourProgress {
  return { completed: {}, dismissed: {}, seenVersion: "" };
}

function load(): TourProgress {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw == null) return empty();
    const parsed = JSON.parse(raw) as Partial<TourProgress> | null;
    return {
      completed: parsed?.completed ?? {},
      dismissed: parsed?.dismissed ?? {},
      seenVersion: typeof parsed?.seenVersion === "string" ? parsed.seenVersion : "",
    };
  } catch {
    return empty();
  }
}

function save(progress: TourProgress): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(progress));
  } catch {
    // storage-less webview: the session still works, progress just won't stick
  }
}

/** First run if we have never recorded a seen version or touched a tour. */
export function shouldShowWelcome(): boolean {
  const progress = load();
  return (
    progress.seenVersion === "" &&
    progress.completed["welcome.v1"] !== true &&
    progress.dismissed["welcome.v1"] !== true
  );
}

/** Compare dot-separated numeric versions; returns <0, 0 or >0. */
export function compareVersions(a: string, b: string): number {
  const parse = (value: string) =>
    value
      .split(/[.-]/)
      .map((part) => Number.parseInt(part, 10))
      .map((part) => (Number.isFinite(part) ? part : 0));
  const left = parse(a);
  const right = parse(b);
  const len = Math.max(left.length, right.length);
  for (let i = 0; i < len; i++) {
    const diff = (left[i] ?? 0) - (right[i] ?? 0);
    if (diff !== 0) return diff;
  }
  return 0;
}

/** Tours shipped after the last seen version that are still unresolved. */
export function pendingUpdateTours(currentVersion: string): Tour[] {
  const progress = load();
  if (progress.seenVersion === "") return [];
  return TOURS.filter(
    (tour) =>
      progress.completed[tour.id] !== true &&
      progress.dismissed[tour.id] !== true &&
      compareVersions(tour.since, currentVersion) <= 0 &&
      compareVersions(tour.since, progress.seenVersion) > 0,
  );
}

export function completeTour(id: string): void {
  const progress = load();
  progress.completed[id] = true;
  save(progress);
}

/** Skipping a tour is permanent (it stays replayable from the TOUR button). */
export function dismissTour(id: string): void {
  const progress = load();
  progress.dismissed[id] = true;
  save(progress);
}

/** Record the app version whose onboarding/update prompt was shown. */
export function markVersionSeen(version: string): void {
  const progress = load();
  progress.seenVersion = version;
  save(progress);
}

/** Replay/reset helper for the library TOUR button. */
export function resetTour(id: string): void {
  const progress = load();
  delete progress.completed[id];
  delete progress.dismissed[id];
  save(progress);
}
