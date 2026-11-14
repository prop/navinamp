/**
 * Svelte action: move a node to the document body.
 *
 * Scaled window roots (`zoom` in the library, `transform: scale()` in the
 * skinned windows) force their descendants to rasterize at 1× and then
 * scale — soft text for the context menu. Portaling to the body lets the
 * menu live in real pixels; it multiplies its own font/spacing by the UI
 * scale instead, so the browser lays the text out at its final size.
 */
export function portal(node: HTMLElement): { destroy(): void } {
  // Keep an anchor in the node's original slot: Svelte's block teardown
  // reconciles against it, so restoring the node before destroy is what keeps
  // the action safe when the menu is ever mounted inside a list/block.
  const anchor = document.createComment("portal-anchor");
  node.before(anchor);
  document.body.appendChild(node);
  return {
    destroy() {
      node.remove();
      if (anchor.parentNode) anchor.replaceWith(node);
    },
  };
}
