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
   */
  import type { ResolvedObject } from "../../lib/skin/resolver";

  interface Props {
    obj: ResolvedObject;
    images: Record<string, string>;
    onaction: (action: string) => void;
    /** Toggle active (SkinSurface maps the action to the App's state). */
    toggled?: boolean;
  }

  let { obj, images, onaction, toggled = false }: Props = $props();

  const o = $derived(
    obj.object.kind === "button" || obj.object.kind === "togglebutton" ? obj.object : null,
  );

  let hover = $state(false);
  let down = $state(false);

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
  }

  function release(e: PointerEvent & { currentTarget: HTMLElement }) {
    if (!down) return;
    down = false;
    // Captured releases arrive here even outside the button; only fire when
    // the release point is actually inside.
    const r = e.currentTarget.getBoundingClientRect();
    const inside =
      e.clientX >= r.left &&
      e.clientX <= r.right &&
      e.clientY >= r.top &&
      e.clientY <= r.bottom;
    const action = o?.action;
    if (inside && action) onaction(action);
  }
</script>

{#if o}
  <div
    class="btn"
    class:hover
    class:down
    class:toggled={obj.object.kind === "togglebutton" && toggled && !activeImage}
    role="button"
    tabindex="0"
    style="left:{obj.x}px; top:{obj.y}px; width:{obj.w ?? 0}px; height:{obj.h ?? 0}px;{image
      ? `background-image:url('${image}');`
      : ''}"
    title={o.tooltip ?? o.id ?? undefined}
    onpointerenter={() => (hover = true)}
    onpointerleave={() => (hover = false)}
    onpointerdown={press}
    onpointerup={release}
    onpointercancel={() => (down = false)}
    onkeydown={(e) => {
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
