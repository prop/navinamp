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
  document.body.appendChild(node);
  return {
    destroy() {
      node.remove();
    },
  };
}
