/**
 * One Escape handler for every webview.
 *
 * Dialogs and transient overlays register a close callback; the most
 * recently registered one wins (surfaces are opened on top of each other,
 * so mount order matches z-order). With nothing registered, Escape hides
 * the current secondary window (playlist / library / eq); the player window
 * stays put.
 *
 * Registration uses a stack, not a pile of window keydown listeners, so
 * nested surfaces (tour → dialog, context menu → dialog) unwind one at a
 * time and an Escape never both closes a dialog and hides its window.
 */

import { invoke } from "@tauri-apps/api/core";

import { IS_PLAYER, LABEL } from "./identity";

type EscapeHandler = () => void;

const stack: EscapeHandler[] = [];
let installed = false;

/** Register a surface's Escape close action. Returns an unregister fn. */
export function registerEscape(handler: EscapeHandler): () => void {
  stack.push(handler);
  let active = true;
  return () => {
    if (!active) return;
    active = false;
    const index = stack.lastIndexOf(handler);
    if (index >= 0) stack.splice(index, 1);
  };
}

/** Install the single window-level listener (idempotent). */
export function installEscapeHandler(): void {
  if (installed) return;
  installed = true;
  window.addEventListener("keydown", (event) => {
    if (event.key !== "Escape" || event.defaultPrevented) return;

    const top = stack[stack.length - 1];
    if (top != null) {
      event.preventDefault();
      top();
      return;
    }

    if (!IS_PLAYER) {
      event.preventDefault();
      void invoke("window_set_visible", { label: LABEL, visible: false }).catch(
        () => undefined,
      );
    }
  });
}
