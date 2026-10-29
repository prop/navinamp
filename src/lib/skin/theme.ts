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
  return theme;
}
