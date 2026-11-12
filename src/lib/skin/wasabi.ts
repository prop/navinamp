/**
 * Wasabi skin-engine semantics shared by the renderer: the object helpers
 * mirror the `impl GuiObject` / `impl Slider` methods in
 * `crates/navinamp-skin/src/model.rs`.
 */

import type { GuiObject, Rect, Slider, Warning } from "./types";

/**
 * The object's position/size rectangle. Unknown ("other") objects have no
 * typed rectangle, so they get an empty one — same as Rust's `DEFAULT_RECT`.
 */
export function objectRect(object: GuiObject): Rect {
  return object.kind === "other" ? {} : object.rect;
}

/** Playback status as reported by the engine (`audio_state.status`). */
export type PlaybackStatus = "stopped" | "playing" | "paused";

/** The slider actions NaVinaMP's engine handles natively (see `SliderAction`). */
export type SliderActionKind = "seek" | "volume" | "balance" | "eq";

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
    case "EQ_SET":
      return "eq";
    default:
      return null;
  }
}

/**
 * The `index` param of an EQ_SET slider — NaVinaMP's mapping (mirrors the
 * .wsz synthesizer, `crates/navinamp-skin/src/wsz.rs` `eq_window`):
 * **0 = preamp, 1..10 = bands 60 Hz … 16 kHz** (band N is `eq_set_band(N-1)`).
 * Null when the param is absent or out of range — such a slider stays inert.
 */
export function eqSliderIndex(slider: Slider): number | null {
  const raw = slider.params?.["index"];
  if (raw == null) return null;
  const n = Number(raw);
  return Number.isInteger(n) && n >= 0 && n <= 10 ? n : null;
}

/** Human-readable form of a parse `Warning` (mirror of its `Display` impl). */
export function formatWarning(warning: Warning): string {
  const v = Object.values(warning)[0];
  const keys = Object.keys(v);
  const detail = keys.map((k) => `${k}=${String(v[k as keyof typeof v])}`).join(" ");
  return `${Object.keys(warning)[0]}(${detail})`;
}

/**
 * Inline CSS override for image payloads. The UI scale decides the sampling:
 *
 *  - at integer scales, force `pixelated`: the 1×/2× mapping is exact
 *    nearest-neighbour, and forcing it guarantees 2× stays bit-identical
 *    to the classic double-size pipeline even if the engine rasterizes an
 *    SVG at intrinsic size instead of at the displayed size;
 *  - at fractional scales, `auto` resamples smoothly on current
 *    Chromium/WebKit (the spec leaves the choice to the UA). This
 *    applies to RASTER payloads too, not just vector ones: nearest-neighbour
 *    at 1.25/1.5 turns a classic skin's 2×2 dither into visible 1px stripes
 *    (the "trail" on dithered seek bars and backgrounds) and makes edges
 *    uneven. Fractional scales therefore soften the bitmap art slightly by
 *    design.
 *
 * Engine note (review finding): Chromium rasterizes SVG at display size
 * already and IGNORES `image-rendering` on SVG documents — the vector part
 * of this override is a measured no-op there. It targets WebKit (WKWebView,
 * the shipping macOS build), whose intrinsic-size rasterization path it
 * corrects.
 *
 * `Number.isInteger(scale)` is only meaningful because `normalizeUiScale`
 * (lib/window/bus.svelte.ts) snaps values to the UI_SCALE_PRESETS list —
 * a future free-form scale slider must snap too, or the integer path
 * silently degrades to `auto`.
 *
 * The call sites interpolate this into a `style` attribute that already
 * contains `url('…')` for the same payload. That stays safe because every
 * payload is base64 (no quotes, parentheses or backslashes); do not start
 * emitting raw percent-encoded SVG URLs without re-escaping those sites.
 */
export function imageRenderingFor(url: string | undefined, scale: number): string {
  if (!url) return "";
  return Number.isInteger(scale) ? "image-rendering:pixelated;" : "image-rendering:auto;";
}
