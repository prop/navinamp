/**
 * Scroll fallback for a window that is smaller than its content.
 *
 * Every window is sized to its content (`sizeWindowTo` in bus.svelte.ts), so
 * normally the page never overflows and `html, body { overflow: hidden }` in
 * App.svelte keeps it still. The two disagree when a window is made smaller
 * than its page: the playlist and the library are resizable, and their
 * content does not reflow, so the page simply overflows. The cut-off part
 * could not be reached at all. While that lasts, the page scrolls.
 *
 * The class is set only once the overflow has lasted a moment. Switching tabs
 * makes the content taller than the window until the OS resize lands, and a
 * scrollbar must not flash for those few frames. It is removed the moment the
 * content fits again.
 */

/** How long the page must overflow the window before it becomes scrollable. */
const SCROLL_DELAY_MS = 400;

/**
 * Sub-pixel rounding under the UI-scale zoom can leave the page a pixel or
 * two taller than the window the content asked for. That is not an overflow.
 */
const TOLERANCE_PX = 4;

/** Class on `<html>` while the page scrolls (see App.svelte). */
const CLIPPED = "clipped";

let timer: ReturnType<typeof setTimeout> | null = null;

/** True when the page is larger than the viewport in either direction. */
function pageOverflows(): boolean {
  const el = document.documentElement;
  return (
    el.scrollWidth > window.innerWidth + TOLERANCE_PX ||
    el.scrollHeight > window.innerHeight + TOLERANCE_PX
  );
}

/** Re-evaluate. Call whenever the content's size or the window's changes. */
function syncOverflowScroll(): void {
  const root = document.documentElement;
  if (!pageOverflows()) {
    if (timer != null) clearTimeout(timer);
    timer = null;
    root.classList.remove(CLIPPED);
    return;
  }
  if (timer != null || root.classList.contains(CLIPPED)) return;
  timer = setTimeout(() => {
    timer = null;
    // The window may have caught up in the meantime (a tab change, a resize).
    if (pageOverflows()) root.classList.add(CLIPPED);
  }, SCROLL_DELAY_MS);
}

/**
 * Watch the page and the viewport; re-evaluate whenever either changes.
 * Returns a cleanup.
 */
export function installOverflowScroll(): () => void {
  const host = document.getElementById("app");
  // The page's own box only tells half the story: content wider than the
  // viewport overflows it without changing it. So the window's root (a direct
  // child of #app, e.g. LibraryWindow's `.win`) is observed as well.
  const observer = new ResizeObserver(syncOverflowScroll);
  const observeContent = () => {
    observer.disconnect();
    if (host instanceof HTMLElement) observer.observe(host);
    for (const el of Array.from(host?.children ?? [])) {
      if (el instanceof HTMLElement) observer.observe(el);
    }
  };
  // A window can swap its root (the playlist switches between its skin and
  // its fallback panel); follow it.
  const swaps = new MutationObserver(observeContent);
  if (host instanceof HTMLElement) {
    swaps.observe(host, { childList: true });
  }
  const onResize = () => syncOverflowScroll();
  window.addEventListener("resize", onResize);
  observeContent();
  syncOverflowScroll();
  return () => {
    observer.disconnect();
    swaps.disconnect();
    window.removeEventListener("resize", onResize);
    if (timer != null) clearTimeout(timer);
    timer = null;
    document.documentElement.classList.remove(CLIPPED);
  };
}
