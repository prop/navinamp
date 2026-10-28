/**
 * Wasabi skin-engine semantics shared by the renderer: the object helpers
 * mirror the `impl GuiObject` / `impl Slider` methods in
 * `crates/navamp-skin/src/model.rs`.
 */

import type { GuiObject, Rect, Slider, Warning } from "./types";

/**
 * The object's position/size rectangle. Unknown ("other") objects have no
 * typed rectangle, so they get an empty one — same as Rust's `DEFAULT_RECT`.
 */
export function objectRect(object: GuiObject): Rect {
  return object.kind === "other" ? {} : object.rect;
}

/** The slider actions NavAmp's engine handles natively (see `SliderAction`). */
export type SliderActionKind = "seek" | "volume" | "balance";

/** Parse the well-known slider actions, case-insensitively (`Slider::action_kind`). */
export function sliderActionKind(slider: Slider): SliderActionKind | null {
  const action = slider.action?.trim().toUpperCase();
  switch (action) {
    case "SEEK":
      return "seek";
    case "VOLUME":
      return "volume";
    case "BALANCE":
      return "balance";
    default:
      return null;
  }
}

/** Human-readable form of a parse `Warning` (mirror of its `Display` impl). */
export function formatWarning(warning: Warning): string {
  const v = Object.values(warning)[0];
  const keys = Object.keys(v);
  const detail = keys.map((k) => `${k}=${String(v[k as keyof typeof v])}`).join(" ");
  return `${Object.keys(warning)[0]}(${detail})`;
}
