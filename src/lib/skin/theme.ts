/**
 * Classic playlist theming: `pledit.txt` colors → editor theme.
 *
 * The `.wsz` synthesizer flattens pledit.txt into `Skin::params`
 * (`pledit.color.{normal,current,normalbg,oldbg,selectedbg}` — see
 * crates/navinamp-skin/src/wsz.rs); this module turns those params into the
 * concrete color set the playlist editor styles itself with. The fallbacks
 * are the classic Winamp base-skin values (pledit green on black), so a
 * skin without pledit.txt — and the built-in fallback skin — still looks
 * right.
 */

/** Colors the playlist editor paints itself with. */
export interface SkinTheme {
  /** Normal row text. */
  normal: string;
  /** Current (playing) row text. */
  current: string;
  /** List background (also the selected row's text backdrop). */
  normalbg: string;
  /** Selected row background. */
  selectedbg: string;
  /** Marker "▶" + drop-indicator color (falls back to `current`). */
  marker?: string;
  /** pledit.txt `Font=` family; absent → the bundled UI fallback. */
  font?: string;
}

/** The classic base-skin pledit palette (also used when a skin has none). */
export const DEFAULT_SKIN_THEME: SkinTheme = {
  normal: "#00ff00",
  current: "#ffffff",
  normalbg: "#000000",
  selectedbg: "#0000c6",
};

function isHexColor(value: string | undefined): value is string {
  return value != null && /^#[0-9a-f]{6}$/i.test(value);
}

/**
 * pledit.txt `Font=` is injected into a CSS font-family string. Accept only
 * a plain family list (letters/digits/spaces/commas/quotes/hyphens) so a
 * hostile skin cannot append further declarations.
 */
function safeFont(value: string | undefined): string | undefined {
  if (value == null) return undefined;
  const font = value.trim();
  if (font === "" || font.length > 80) return undefined;
  return /^[A-Za-z0-9][A-Za-z0-9 ,'"-]*$/.test(font) ? font : undefined;
}

/** Blend two `#rrggbb` colors; `amount` 0 = a, 1 = b. */
export function mixHex(a: string, b: string, amount: number): string {
  if (!isHexColor(a) || !isHexColor(b)) return a;
  const t = Math.max(0, Math.min(1, amount));
  const channel = (hex: string, i: number) =>
    parseInt(hex.slice(1 + i * 2, 3 + i * 2), 16);
  const mix = (i: number) => Math.round(channel(a, i) + (channel(b, i) - channel(a, i)) * t);
  return `#${[0, 1, 2].map((i) => mix(i).toString(16).padStart(2, "0")).join("")}`;
}

/**
 * Build a theme from a skin's params bag: known-good `pledit.color.*`
 * entries override the defaults one by one; anything malformed (or absent)
 * keeps the default. Unknown keys are ignored.
 */
export function themeFromSkinParams(
  params: Record<string, string> | undefined,
): SkinTheme {
  const theme: SkinTheme = { ...DEFAULT_SKIN_THEME };
  if (!params) return theme;
  const pick = (key: keyof SkinTheme, param: string): void => {
    const value = params[`pledit.color.${param}`];
    if (isHexColor(value)) theme[key] = value as string;
  };
  pick("normal", "normal");
  pick("current", "current");
  pick("normalbg", "normalbg");
  pick("selectedbg", "selectedbg");
  const font = safeFont(params["pledit.font"]);
  if (font) theme.font = font;
  return theme;
}
