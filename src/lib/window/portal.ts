/**
 * Svelte action: move a node to the document body.
 *
 * Scaled window roots (`zoom` in the library, `transform: scale()` in the
 * skinned windows) force their descendants to rasterize at 1× and then
 * scale — soft text for the context menu. Portaling to the body lets the
 * menu live in real pixels; it multiplies its own font/spacing by the UI
 * scale instead, so the browser lays the text out at its final size.
 *
 * Teardown order matters: when the component is removed, Svelte first
 * detaches the nodes it tracks — wherever they are, so from the body here —
 * and only then runs this action's cleanup. The cleanup must therefore take
 * the node out for good. Moving it back to its original slot re-attached it
 * to a block that had just been torn down: every dismissed menu stayed in
 * the window's scaled root as a stale, still-clickable copy drawn at the UI
 * scale twice.
 */
export function portal(node: HTMLElement): { destroy(): void } {
  document.body.appendChild(node);
  return {
    destroy() {
      node.remove();
    },
  };
}
