/**
 * Drag-to-move for custom title strips (Svelte action).
 *
 * Tauri's `data-tauri-drag-region` starts the OS drag loop on `mousedown` and
 * reserves double-click for window maximize. That steals the windowshade
 * gesture (and maximizes the window) on skinned title bars, so instead we
 * start the drag only after the pointer actually moves. A double-click without
 * movement therefore stays in the page and folds the window.
 */

import { getCurrentWindow } from "@tauri-apps/api/window";

/** Pointer travel (CSS px) before a press becomes a window drag. */
const MOVE_THRESHOLD = 4;

const IN_TAURI =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export function dragWindow(node: HTMLElement): { destroy: () => void } {
  let pending = false;
  let dragging = false;
  let startX = 0;
  let startY = 0;

  function onPointerDown(e: PointerEvent): void {
    if (e.button !== 0) return;
    pending = true;
    dragging = false;
    startX = e.clientX;
    startY = e.clientY;
  }

  function onPointerMove(e: PointerEvent): void {
    if (!pending || dragging) return;
    if (Math.hypot(e.clientX - startX, e.clientY - startY) < MOVE_THRESHOLD) return;
    dragging = true;
    if (IN_TAURI) {
      void getCurrentWindow().startDragging().catch(() => undefined);
    }
  }

  function onPointerUp(): void {
    pending = false;
  }

  node.addEventListener("pointerdown", onPointerDown);
  window.addEventListener("pointermove", onPointerMove);
  window.addEventListener("pointerup", onPointerUp);

  return {
    destroy(): void {
      node.removeEventListener("pointerdown", onPointerDown);
      window.removeEventListener("pointermove", onPointerMove);
      window.removeEventListener("pointerup", onPointerUp);
    },
  };
}
