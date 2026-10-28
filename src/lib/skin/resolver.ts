/**
 * Layout resolver: flattens one container layout into a render list.
 *
 * `<group id="..."/>` instances are replaced by the objects of the
 * `<groupdef>` they reference, recursively, with each object's absolute
 * position = accumulated group-instance offsets + its own rect. Cycles and
 * dangling ids are skipped (and reported), and nesting is capped so a
 * hostile skin cannot make us recurse forever.
 */

import type { GuiObject, SkinPayload } from "./types";
import { objectRect } from "./wasabi";

/** One renderable object with its absolute position. */
export interface ResolvedObject {
  object: GuiObject;
  /** Absolute left/top: group-instance offsets + the object's own rect. */
  x: number;
  y: number;
  /** Size from the object's rect; undefined = attribute absent. */
  w?: number;
  h?: number;
}

/** A layout flattened into an absolutely-positioned render list. */
export interface ResolvedLayout {
  containerId: string;
  layoutId: string;
  /** Window size in skin pixels. */
  w: number;
  h: number;
  /** Render list; z-order = document order. */
  objects: ResolvedObject[];
  /** bitmap id -> data URL, inherited from the skin payload. */
  images: Record<string, string>;
  /** Resolution problems (cycles, dangling ids, depth) found while flattening. */
  warnings: string[];
}

/** Safety cap on `<group>` nesting, mirroring the parser's depth limit. */
const MAX_DEPTH = 16;

/**
 * Resolve a layout for rendering. `containerId`/`layoutId` select a specific
 * window mode; when omitted (or unmatched) the first container with layouts
 * and its first layout are used.
 */
export function resolveLayout(
  payload: SkinPayload,
  containerId?: string,
  layoutId?: string,
): ResolvedLayout | null {
  const { skin, images } = payload;
  const container =
    skin.containers.find((c) => c.id === containerId) ??
    skin.containers.find((c) => c.layouts.length > 0);
  if (!container) return null;
  const layout =
    container.layouts.find((l) => l.id === layoutId) ?? container.layouts[0];
  if (!layout) return null;

  const warnings: string[] = [];
  const objects: ResolvedObject[] = [];

  const walk = (
    list: readonly GuiObject[],
    ox: number,
    oy: number,
    depth: number,
    chain: readonly string[],
  ): void => {
    for (const object of list) {
      if (object.kind !== "group") {
        const rect = objectRect(object);
        objects.push({
          object,
          x: ox + (rect.x ?? 0),
          y: oy + (rect.y ?? 0),
          w: rect.w,
          h: rect.h,
        });
        continue;
      }

      // Group instance: splice in the referenced groupdef's objects.
      const gid = object.id;
      const where = chain.length > 0 ? chain.join(" -> ") : "<layout>";
      if (!gid) {
        warnings.push(`group instance without id inside ${where}; skipped`);
        continue;
      }
      const def = skin.groups[gid];
      if (!def) {
        warnings.push(`group instance '${gid}' inside ${where} has no matching groupdef; skipped`);
        continue;
      }
      if (chain.includes(gid)) {
        warnings.push(`group cycle: ${[...chain, gid].join(" -> ")}; skipped`);
        continue;
      }
      if (depth >= MAX_DEPTH) {
        warnings.push(`group nesting deeper than ${MAX_DEPTH} levels at '${gid}'; skipped`);
        continue;
      }
      const gx = ox + (object.rect.x ?? 0);
      const gy = oy + (object.rect.y ?? 0);
      const gw = object.rect.w ?? def.rect.w;
      const gh = object.rect.h ?? def.rect.h;

      // The groupdef's optional background becomes a layer under its children.
      if (def.background) {
        objects.push({
          object: {
            kind: "layer",
            id: `${gid}.background`,
            image: def.background,
            rect: { w: gw, h: gh },
            params: {},
          },
          x: gx,
          y: gy,
          w: gw,
          h: gh,
        });
      }
      walk(def.objects, gx, gy, depth + 1, [...chain, gid]);
    }
  };

  walk(layout.objects, 0, 0, 0, []);

  // Fall back to the render list's bounds when the layout rect has no size.
  const w =
    layout.rect.w ?? objects.reduce((max, o) => Math.max(max, o.x + (o.w ?? 0)), 0);
  const h =
    layout.rect.h ?? objects.reduce((max, o) => Math.max(max, o.y + (o.h ?? 0)), 0);

  return {
    containerId: container.id,
    layoutId: layout.id,
    w,
    h,
    objects,
    images,
    warnings,
  };
}
