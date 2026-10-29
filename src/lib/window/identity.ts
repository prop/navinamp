/**
 * Window identity (M6b): which of the three OS windows is this webview?
 *
 * Labels (frozen contract): "main" = player window and the SINGLE OWNER of
 * queue/playback state, "playlist" = Playlist Editor view, "library" =
 * Media Library + EQ + settings + debug.
 *
 * Detection is deliberately forgiving so a wrong label (or running outside
 * a Tauri webview, e.g. plain `vite dev` in a browser) degrades to the
 * player view instead of a blank window.
 */

import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

export const WINDOW_LABELS = ["main", "playlist", "library"] as const;
export type WindowLabel = (typeof WINDOW_LABELS)[number];

/** The player window's label — owner of the queue module. */
export const PLAYER_LABEL: WindowLabel = "main";

function detectLabel(): WindowLabel {
  try {
    const label = getCurrentWebviewWindow().label;
    return (WINDOW_LABELS as readonly string[]).includes(label)
      ? (label as WindowLabel)
      : PLAYER_LABEL; // unknown label → degrade to the player view
  } catch {
    return PLAYER_LABEL; // no Tauri internals (browser dev preview, tests)
  }
}

/** This webview's window label (never unknown — see detectLabel). */
export const LABEL: WindowLabel = detectLabel();

/** True only in the player window: queue module + audio pollers live here. */
export const IS_PLAYER: boolean = LABEL === PLAYER_LABEL;
