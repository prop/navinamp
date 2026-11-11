<script lang="ts">
  /**
   * `<button>` and `<togglebutton>`: clickable objects wired to a standard
   * Wasabi action (PLAY, PAUSE, ...).
   *
   * Three-state background: downimage while pressed, hoverimage while
   * hovered, image otherwise — each falling back to the normal image (and,
   * one day, to a disabled look; the model carries no disabled image).
   * togglebutton: while its toggle is active (`toggled`, fed by SkinSurface
   * from the App's state, e.g. TOGGLE_PLAYLIST / TOGGLE_SHUFFLE) the
   * skin's dedicated active cell (`params["activeimage"]`, shufrep.bmp's
   * third row) stays rendered — the lit glyph, not the momentary pressed
   * one. Skins without an activeimage fall back to downimage.
   *
   * Transport buttons can also be held: after HOLD_DELAY_MS the component
   * reports `onholdstart` and suppresses the eventual click; `onholdend`
   * fires on release/cancel. A hold that the owner declines (false) falls
   * back to a normal click.
   */
  import { onDestroy } from "svelte";

  import type { ResolvedObject } from "../../lib/skin/resolver";
  import { imageRenderingFor } from "../../lib/skin/wasabi";

  interface Props {
    obj: ResolvedObject;
    images: Record<string, string>;
    onaction: (action: string) => void;
    /** Toggle active (SkinSurface maps the action to the App's state). */
    toggled?: boolean;
    /** Whether this action has press-and-hold behavior (PREV/NEXT). */
    holdable?: boolean;
    /** Begin a hold; return false when the owner can't seek right now. */
    onholdstart?: (action: string) => boolean;
    /** End a hold that actually started. */
    onholdend?: (action: string) => void;
    /** Global UI scale; integer values rasterize vector payloads nearest-neighbour. */
    scale?: number;
  }

  let {
    obj,
    images,
    onaction,
    toggled = false,
    holdable = false,
    onholdstart,
    onholdend,
    scale = 1,
  }: Props = $props();

  /** Delay before a press becomes a hold instead of a click. */
  const HOLD_DELAY_MS = 250;

  let holdTimer: ReturnType<typeof setTimeout> | null = null;
  let holdActive = false;
  let holdAction: string | null = null;

  function clearHoldTimer(): void {
    if (holdTimer != null) {
      clearTimeout(holdTimer);
      holdTimer = null;
    }
  }

  function endHold(): void {
    const action = holdAction;
    const wasHolding = holdActive;
    holdAction = null;
    holdActive = false;
    clearHoldTimer();
    if (wasHolding && action) onholdend?.(action);
  }

  onDestroy(endHold);

  const o = $derived(
    obj.object.kind === "button" || obj.object.kind === "togglebutton" ? obj.object : null,
  );

  let hover = $state(false);
  let down = $state(false);

  // Losing focus also loses the pointer-up; cancel a pending/active hold.
  $effect(() => {
    if (!holdable) return;
    const cancel = () => {
      down = false;
      endHold();
    };
    window.addEventListener("blur", cancel);
    return () => window.removeEventListener("blur", cancel);
  });

  // The togglebutton's lit cell. The model has no typed `activeimage` (it
  // rides in `params`, where a Wasabi parser would keep unknown attributes).
  const activeImage = $derived.by(() => {
    if (obj.object.kind !== "togglebutton") return undefined;
    const key = obj.object.params?.["activeimage"];
    return key ? images[key] : undefined;
  });

  const image = $derived.by(() => {
    if (!o) return undefined;
    if (toggled && activeImage) return activeImage;
    const pressed = down || (obj.object.kind === "togglebutton" && toggled);
    if (pressed && o.downimage && images[o.downimage]) return images[o.downimage];
    if (hover && o.hoverimage && images[o.hoverimage]) return images[o.hoverimage];
    return o.image ? images[o.image] : undefined;
  });

  function press(e: PointerEvent & { currentTarget: HTMLElement }) {
    down = true;
    // Capture so the press tracks even if the pointer leaves the button.
    e.currentTarget.setPointerCapture(e.pointerId);
    const action = o?.action;
    if (!holdable || !action || !onholdstart || !onholdend) return;
    clearHoldTimer();
    holdAction = action;
    holdActive = false;
    holdTimer = setTimeout(() => {
      holdTimer = null;
      if (holdAction !== action) return;
      // The owner may refuse (stopped engine, unknown duration); in that
      // case leave holdActive false so release still performs the click.
      holdActive = onholdstart(action) === true;
    }, HOLD_DELAY_MS);
  }

  function release(e: PointerEvent & { currentTarget: HTMLElement }) {
    if (!down) return;
    down = false;
    const action = holdAction;
    const wasHolding = holdActive;
    holdAction = null;
    holdActive = false;
    clearHoldTimer();
    if (wasHolding && action) {
      // A completed hold is a seek gesture, not a track change.
      onholdend?.(action);
      return;
    }
    // Captured releases arrive here even outside the button; only fire when
    // the release point is actually inside.
    const r = e.currentTarget.getBoundingClientRect();
    const inside =
      e.clientX >= r.left &&
      e.clientX <= r.right &&
      e.clientY >= r.top &&
      e.clientY <= r.bottom;
    const clicked = o?.action;
    if (inside && clicked) onaction(clicked);
  }

  function cancelPress(): void {
    down = false;
    endHold();
  }
</script>

{#if o}
  <div
    class="btn"
    class:bare={!image}
    class:hover
    class:down
    class:toggled={obj.object.kind === "togglebutton" && toggled && !activeImage}
    role="button"
    tabindex="0"
    style="left:{obj.x}px; top:{obj.y}px; width:{obj.w ?? 0}px; height:{obj.h ?? 0}px;{image
      ? `background-image:url('${image}');`
      : ''}{imageRenderingFor(image, scale)}"
    title={o.tooltip ?? o.id ?? undefined}
    onpointerenter={() => (hover = true)}
    onpointerleave={() => (hover = false)}
    onpointerdown={press}
    onpointerup={release}
    onpointercancel={cancelPress}
    onkeydown={(e) => {
      // Keyboard activation stays a one-shot action; hold seek is pointer-only.
      if (e.key !== "Enter" && e.key !== " ") return;
      e.preventDefault();
      if (o?.action) onaction(o.action);
    }}
  ></div>
{/if}

<style>
  .btn {
    position: absolute;
    background-size: 100% 100%;
    background-repeat: no-repeat;
    background-color: #2a2a2e;
    outline: 1px solid #555;
    cursor: pointer;
    user-select: none;
    touch-action: none;
  }

  /* Image-less buttons are transparent hotspots over artwork the skin has
     already painted (classic pledit's baked ADD/REM/SEL/MISC + titlebar
     glyphs). Keep the hit area and focus ring, drop the fallback chrome. */
  .btn.bare {
    background-color: transparent;
    outline: none;
  }

  .btn:focus-visible {
    outline: 1px solid #3ccb4e;
  }

  .btn.down {
    filter: brightness(0.85);
  }

  /* Toggled without an activeimage (or a downimage): at least look held down. */
  .btn.toggled {
    filter: brightness(0.85);
  }
</style>
