/**
 * Titlebar band + drag-region math (M6b).
 *
 * The skin — not the app — draws the window chrome, so the draggable band
 * must be discovered from the skin model. Detection, in order:
 *
 *  1. a `<group id="player.titlebar">` instance in the CURRENT layout → its
 *     rect is the band (the fallback skin's player/playlist titlebars, and
 *     any third-party skin following the same convention);
 *  2. a short layout (≤ SHADE_STRIP_H) is a shade strip → the whole window
 *     is the band;
 *  3. the classic constant: y 0..14 across the window width (the fallback
 *     skin's titlebar bitmap is 275×14, ink divider at y=13).
 *
 * Interactive objects inside the band (buttons, togglebuttons, sliders,
 * click-to-cycle vis canvases) punch holes through it: the band minus their
 * x-spans (a 1D interval complement — every interactive object spans the
 * full band height in practice) leaves one drag rectangle per remaining
 * x-span, so the window buttons stay clickable and everything else drags.
 */

import type { GuiObject, Rect, Skin } from "./types";
import type { ResolvedLayout } from "./resolver";

/** Convention id of a skin's player titlebar group (fallback skin uses it). */
export const TITLEBAR_GROUP_ID = "player.titlebar";
/** Classic Wasabi titlebar height (fallback skin: 275×14, divider at y=13). */
export const CLASSIC_TITLEBAR_H = 14;
/** Layouts at most this tall are treated as shade strips. */
export const SHADE_STRIP_H = 16;

export interface DragRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

type GroupInstanceObj = Extract<GuiObject, { kind: "group" }>;

function isTitlebarInstance(o: GuiObject): o is GroupInstanceObj {
  return o.kind === "group" && o.id === TITLEBAR_GROUP_ID;
}

function titlebarGroupRect(skin: Skin, resolved: ResolvedLayout): Rect | null {
  const container = skin.containers.find((c) => c.id === resolved.containerId);
  const layout = container?.layouts.find((l) => l.id === resolved.layoutId);
  const instance = layout?.objects.find(isTitlebarInstance);
  if (!instance || (instance.rect.w ?? 0) <= 0 || (instance.rect.h ?? 0) <= 0) return null;
  return instance.rect;
}

/** Objects the user can click/drag inside the band — they need holes. */
function isInteractive(kind: string): boolean {
  return kind === "button" || kind === "togglebutton" || kind === "slider" || kind === "vis";
}

/**
 * The draggable rectangles for one resolved layout, in skin pixels (the
 * caller scales them). Slivers narrower than 3px are dropped — the 1-2px
 * gaps between adjacent buttons are unusable as drag targets.
 */
export function dragRects(skin: Skin, resolved: ResolvedLayout): DragRect[] {
  const band =
    titlebarGroupRect(skin, resolved) ??
    (resolved.h <= SHADE_STRIP_H
      ? { x: 0, y: 0, w: resolved.w, h: resolved.h }
      : { x: 0, y: 0, w: resolved.w, h: CLASSIC_TITLEBAR_H });

  const x0 = band.x ?? 0;
  const x1 = x0 + (band.w ?? resolved.w);
  const y0 = band.y ?? 0;
  const y1 = y0 + (band.h ?? CLASSIC_TITLEBAR_H);

  const spans: [number, number][] = [];
  for (const o of resolved.objects) {
    if (!isInteractive(o.object.kind)) continue;
    if (o.y + (o.h ?? 0) <= y0 || o.y >= y1) continue; // no vertical overlap
    const a = Math.max(x0, o.x);
    const b = Math.min(x1, o.x + (o.w ?? 0));
    if (b > a) spans.push([a, b]);
  }
  spans.sort((p, q) => p[0] - q[0]);

  const rects: DragRect[] = [];
  const push = (a: number, b: number): void => {
    if (b - a < 3) return;
    rects.push({ x: a, y: y0, w: b - a, h: y1 - y0 });
  };
  let cursor = x0;
  for (const [a, b] of spans) {
    if (a > cursor) push(cursor, a);
    cursor = Math.max(cursor, b);
  }
  if (cursor < x1) push(cursor, x1);
  return rects;
}
