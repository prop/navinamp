<script lang="ts">
  /**
   * Renders one resolved layout: absolutely-positioned objects in document
   * order (z-order = document order), inside a container sized to the layout
   * rect. An integer `scale` factor (1x default, 2x for retina) is applied
   * with a CSS transform from the top-left corner; the frame reserves the
   * scaled space.
   *
   * Playback values (`status`/`position`/`duration`/`volume`) are fed by the
   * caller's 10 Hz audio_state poll; `ticker` is the songticker content
   * (queue.currentItem). Togglebutton on-states flow down as
   * `playlistVisible` / `shadeActive`; the callbacks wire the skinned
   * controls to IPC.
   */
  import type { ResolvedLayout, ResolvedObject } from "../../lib/skin/resolver";
  import type { GuiObject } from "../../lib/skin/types";
  import { sliderActionKind, type PlaybackStatus } from "../../lib/skin/wasabi";
  import SkinButton from "./SkinButton.svelte";
  import SkinLayer from "./SkinLayer.svelte";
  import SkinPlaceholder from "./SkinPlaceholder.svelte";
  import SkinSlider from "./SkinSlider.svelte";
  import SkinText from "./SkinText.svelte";
  import SkinVis from "./SkinVis.svelte";

  interface Props {
    layout: ResolvedLayout;
    scale?: number;
    status: PlaybackStatus;
    position: number;
    duration: number;
    volume: number;
    /** Songticker content ("Artist - Title" from queue.currentItem). */
    ticker: string;
    /** TOGGLE_PLAYLIST on-state (App's playlist editor visibility). */
    playlistVisible: boolean;
    /** TOGGLE_SHADE on-state (M6 placeholder: App's media-library visibility). */
    shadeActive: boolean;
    onaction: (action: string) => void;
    onseek: (secs: number) => void;
    onvolume: (v: number) => void;
  }

  let {
    layout,
    scale = 1,
    status,
    position,
    duration,
    volume,
    ticker,
    playlistVisible,
    shadeActive,
    onaction,
    onseek,
    onvolume,
  }: Props = $props();

  // Per-slider wiring, decided by the Wasabi `action` attribute. The helpers
  // take a GuiObject (templates cannot keep narrowing inside callbacks) and
  // narrow themselves.
  function sliderValue(object: GuiObject): number {
    if (object.kind !== "slider") return 0.5;
    switch (sliderActionKind(object)) {
      case "volume":
        return volume;
      case "seek":
        return duration > 0 ? Math.min(position / duration, 1) : 0;
      // Balance has no engine-side pan yet — park it centered.
      default:
        return 0.5;
    }
  }

  function sliderDisabled(object: GuiObject): boolean {
    return object.kind === "slider" && sliderActionKind(object) === "seek" && duration <= 0;
  }

  // Seek commits on release; volume applies live while dragging.
  function sliderLive(object: GuiObject, v: number): void {
    if (object.kind === "slider" && sliderActionKind(object) === "volume") onvolume(v);
  }

  function sliderCommit(object: GuiObject, v: number): void {
    if (object.kind !== "slider") return;
    switch (sliderActionKind(object)) {
      case "seek":
        onseek(v * duration);
        break;
      case "volume":
        onvolume(v);
        break;
      // TODO: balance — needs engine-side pan support.
      default:
        break;
    }
  }

  // Groups were resolved away by the resolver; "other" nodes are not painted.
  function isPainted(obj: ResolvedObject): boolean {
    return obj.object.kind !== "group" && obj.object.kind !== "other";
  }

  // Togglebutton on-state: the App's visibility toggles, matched by action.
  function toggleState(object: GuiObject): boolean {
    if (object.kind !== "togglebutton") return false;
    switch (object.action?.trim().toUpperCase()) {
      case "TOGGLE_PLAYLIST":
        return playlistVisible;
      case "TOGGLE_SHADE":
        return shadeActive;
      default:
        return false;
    }
  }
</script>

<div class="frame" style="width:{layout.w * scale}px; height:{layout.h * scale}px;">
  <div
    class="surface"
    style="width:{layout.w}px; height:{layout.h}px; transform:scale({scale});"
  >
    {#each layout.objects as obj, i (i)}
      {#if !isPainted(obj)}
        <!-- skipped -->
      {:else if obj.object.kind === "layer" || obj.object.kind === "animatedlayer"}
        <SkinLayer {obj} images={layout.images} {status} />
      {:else if obj.object.kind === "button" || obj.object.kind === "togglebutton"}
        <SkinButton {obj} images={layout.images} {onaction} toggled={toggleState(obj.object)} />
      {:else if obj.object.kind === "slider"}
        <SkinSlider
          {obj}
          images={layout.images}
          value={sliderValue(obj.object)}
          disabled={sliderDisabled(obj.object)}
          onlive={(v) => sliderLive(obj.object, v)}
          oncommit={(v) => sliderCommit(obj.object, v)}
        />
      {:else if obj.object.kind === "text"}
        <SkinText {obj} {status} {position} {ticker} />
      {:else if obj.object.kind === "vis"}
        <SkinVis {obj} {status} />
      {:else}
        <SkinPlaceholder {obj} />
      {/if}
    {/each}
  </div>
</div>

<style>
  .frame {
    overflow: hidden;
  }

  .surface {
    position: relative;
    transform-origin: top left;
    image-rendering: pixelated;
  }
</style>
