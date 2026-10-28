<script lang="ts">
  /**
   * `<slider>`: seek/volume/balance dragger.
   *
   * Track background: downimage while dragging, hoverimage while hovered,
   * image otherwise (with fallbacks). Thumb: `thumbimage` stretched onto a
   * square sized by the track's cross dimension. Orientation comes from
   * params (`orientation="vertical|horizontal"`, default horizontal; value
   * 0 is at the left for horizontal, at the bottom for vertical).
   *
   * Pointer drag uses pointer capture; `onlive` fires continuously while
   * dragging (volume sets it live), `oncommit` once on release.
   */
  import type { ResolvedObject } from "../../lib/skin/resolver";

  interface Props {
    obj: ResolvedObject;
    images: Record<string, string>;
    /** Current value, 0..1 (engine-fed when not dragging). */
    value: number;
    disabled?: boolean;
    /** Called continuously while dragging; optional (volume only). */
    onlive?: (v: number) => void;
    /** Called once on release (and on keyboard nudges) with the final value. */
    oncommit: (v: number) => void;
  }

  let { obj, images, value, disabled = false, onlive, oncommit }: Props = $props();

  const o = $derived(obj.object.kind === "slider" ? obj.object : null);
  const vertical = $derived(o?.params?.["orientation"] === "vertical");

  let hover = $state(false);
  let dragging = $state(false);
  let dragValue = $state(0);

  const shown = $derived(dragging ? dragValue : Math.min(1, Math.max(0, value)));

  const trackImage = $derived.by(() => {
    if (!o) return undefined;
    if (dragging && o.downimage && images[o.downimage]) return images[o.downimage];
    if (hover && o.hoverimage && images[o.hoverimage]) return images[o.hoverimage];
    return o.image ? images[o.image] : undefined;
  });

  const thumbImage = $derived(o?.thumbimage ? images[o.thumbimage] : undefined);

  // Square thumb, sized by the track's cross dimension.
  const thumbSize = $derived(vertical ? (obj.w ?? 12) : (obj.h ?? 12));

  function fraction(e: PointerEvent & { currentTarget: HTMLElement }): number {
    // getBoundingClientRect is in *scaled* screen px and so is clientX/Y,
    // so the fraction is scale-independent.
    const r = e.currentTarget.getBoundingClientRect();
    const f = vertical
      ? 1 - (e.clientY - r.top) / r.height
      : (e.clientX - r.left) / r.width;
    return Number.isFinite(f) ? Math.min(1, Math.max(0, f)) : 0;
  }

  function down(e: PointerEvent & { currentTarget: HTMLElement }) {
    if (disabled) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    dragging = true;
    dragValue = fraction(e);
    onlive?.(dragValue);
  }

  function move(e: PointerEvent & { currentTarget: HTMLElement }) {
    if (!dragging) return;
    dragValue = fraction(e);
    onlive?.(dragValue);
  }

  function up(e: PointerEvent & { currentTarget: HTMLElement }) {
    if (!dragging) return;
    dragging = false;
    oncommit(fraction(e));
  }

  function key(e: KeyboardEvent) {
    if (disabled) return;
    let v: number | null = null;
    if (e.key === "ArrowRight" || e.key === "ArrowUp") v = Math.min(1, shown + 0.05);
    else if (e.key === "ArrowLeft" || e.key === "ArrowDown") v = Math.max(0, shown - 0.05);
    else if (e.key === "Home") v = 0;
    else if (e.key === "End") v = 1;
    if (v == null) return;
    e.preventDefault();
    onlive?.(v);
    oncommit(v);
  }
</script>

{#if o}
  <div
    class="track"
    class:hover
    class:dragging
    class:disabled
    role="slider"
    aria-label={o.id ?? "slider"}
    aria-orientation={vertical ? "vertical" : "horizontal"}
    aria-valuemin="0"
    aria-valuemax="1"
    aria-valuenow={Math.round(shown * 100) / 100}
    tabindex="0"
    style="left:{obj.x}px; top:{obj.y}px; width:{obj.w ?? 0}px; height:{obj.h ?? 0}px;{trackImage
      ? `background-image:url('${trackImage}');`
      : ''}"
    onpointerenter={() => (hover = true)}
    onpointerleave={() => (hover = false)}
    onpointerdown={down}
    onpointermove={move}
    onpointerup={up}
    onpointercancel={() => (dragging = false)}
    onkeydown={key}
  ></div>
  <div
    class="thumb"
    class:disabled
    style="left:{vertical
        ? obj.x + ((obj.w ?? 0) - thumbSize) / 2
        : obj.x + shown * ((obj.w ?? 0) - thumbSize)}px;
      top:{vertical
        ? obj.y + (1 - shown) * ((obj.h ?? 0) - thumbSize)
        : obj.y + ((obj.h ?? 0) - thumbSize) / 2}px;
      width:{thumbSize}px; height:{thumbSize}px;{thumbImage
        ? `background-image:url('${thumbImage}');`
        : ''}"
  ></div>
{/if}

<style>
  .track {
    position: absolute;
    background-size: 100% 100%;
    background-repeat: no-repeat;
    background-color: #1a1a20;
    outline: 1px solid #444;
    cursor: pointer;
    touch-action: none;
  }

  .track:focus-visible {
    outline: 1px solid #3ccb4e;
  }

  .track.disabled {
    cursor: default;
    opacity: 0.6;
  }

  .thumb {
    position: absolute;
    background-size: 100% 100%;
    background-repeat: no-repeat;
    background-color: #6a6a76;
    outline: 1px solid #c8c8d0;
    pointer-events: none;
  }

  .thumb.disabled {
    opacity: 0.6;
  }
</style>
