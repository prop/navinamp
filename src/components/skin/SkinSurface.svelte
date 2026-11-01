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
   * `playlistVisible` (the playlist window's WinInfo.visible) /
   * `shadeActive` (the player window's layout === "shade") / `eqVisible`
   * (the EQ window's visibility) / `eq` (the EQ window's live state —
   * enabled, preamp, bands, auto); the callbacks wire the skinned controls
   * to the owning window's action executor.
   */
  import type { ResolvedLayout, ResolvedObject } from "../../lib/skin/resolver";
  import type { GuiObject } from "../../lib/skin/types";
  import { eqSliderIndex, sliderActionKind, type PlaybackStatus } from "../../lib/skin/wasabi";
  import { dbToFraction, fractionToDb, type EqLive } from "../../lib/audio/eq";
  import SkinButton from "./SkinButton.svelte";
  import SkinLayer from "./SkinLayer.svelte";
  import SkinPlaceholder from "./SkinPlaceholder.svelte";
  import SkinSlider from "./SkinSlider.svelte";
  import SkinStatus from "./SkinStatus.svelte";
  import SkinText from "./SkinText.svelte";
  import SkinVis from "./SkinVis.svelte";

  interface Props {
    layout: ResolvedLayout;
    scale?: number;
    /** Playback feed (player's 10 Hz poll); defaults render idle controls. */
    status?: PlaybackStatus;
    position?: number;
    duration?: number;
    volume?: number;
    /** Engine track metadata for the classic kbps/kHz and mono/stereo readouts. */
    bitrateKbps?: number | null;
    sampleRateHz?: number | null;
    channels?: number | null;
    /** Songticker content ("Artist - Title" from queue.currentItem). */
    ticker?: string;
    /** TOGGLE_PLAYLIST on-state (the playlist window's visibility). */
    playlistVisible?: boolean;
    /** TOGGLE_SHADE on-state (the player window's shade layout is active). */
    shadeActive?: boolean;
    /** TOGGLE_EQ on-state (the EQ window's visibility — player skin button). */
    eqVisible?: boolean;
    /**
     * Live EQ state (the EQ window feeds its 2 Hz `eq_state` poll). Absent
     * → EQ_SET sliders render parked and EQ toggles render off.
     */
    eq?: EqLive | null;
    onaction: (action: string) => void;
    onseek?: (secs: number) => void;
    onvolume?: (v: number) => void;
    /**
     * EQ_SET slider apply (fires on every drag tick AND on commit, like the
     * volume slider). `skinIndex` is the slider's `index` param: 0 = preamp,
     * 1..10 = bands 60 Hz…16 kHz (see `eqSliderIndex`).
     */
    oneqset?: (skinIndex: number, db: number) => void;
  }

  let {
    layout,
    scale = 1,
    status = "stopped",
    position = 0,
    duration = 0,
    volume = 0.5,
    bitrateKbps = null,
    sampleRateHz = null,
    channels = null,
    ticker = "",
    playlistVisible = false,
    shadeActive = false,
    eqVisible = false,
    eq = null,
    onaction,
    onseek,
    onvolume,
    oneqset,
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
      case "eq": {
        if (!eq) return 0.5;
        const i = eqSliderIndex(object);
        if (i == null) return 0.5;
        return dbToFraction(i === 0 ? eq.preampDb : (eq.bandsDb[i - 1] ?? 0));
      }
      // Balance has no engine-side pan yet — park it centered.
      default:
        return 0.5;
    }
  }

  function sliderDisabled(object: GuiObject): boolean {
    return object.kind === "slider" && sliderActionKind(object) === "seek" && duration <= 0;
  }

  // Seek commits on release; volume (and EQ bands) apply live while dragging.
  function sliderLive(object: GuiObject, v: number): void {
    if (object.kind !== "slider") return;
    const kind = sliderActionKind(object);
    if (kind === "volume") onvolume?.(v);
    else if (kind === "eq") eqApply(object, v);
  }

  function sliderCommit(object: GuiObject, v: number): void {
    if (object.kind !== "slider") return;
    switch (sliderActionKind(object)) {
      case "seek":
        onseek?.(v * duration);
        break;
      case "volume":
        onvolume?.(v);
        break;
      case "eq":
        eqApply(object, v);
        break;
      // TODO: balance — needs engine-side pan support.
      default:
        break;
    }
  }

  // Fire-and-forget EQ apply; the EQ window's poll reconciles the value.
  function eqApply(object: GuiObject, fraction: number): void {
    const i = object.kind === "slider" ? eqSliderIndex(object) : null;
    if (i != null) oneqset?.(i, Math.round(fractionToDb(fraction)));
  }

  // Groups were resolved away by the resolver; "other" nodes are not painted.
  function isPainted(obj: ResolvedObject): boolean {
    return obj.object.kind !== "group" && obj.object.kind !== "other";
  }

  // Togglebutton on-state: the App's visibility toggles + the EQ window's
  // live state, matched by action. EQ_TOGGLE/EQ_AUTO are the canonical
  // names (the .wsz synthesizer emits them); ON/AUTO are accepted aliases.
  function toggleState(object: GuiObject): boolean {
    if (object.kind !== "togglebutton") return false;
    switch (object.action?.trim().toUpperCase()) {
      case "TOGGLE_PLAYLIST":
        return playlistVisible;
      case "TOGGLE_SHADE":
        return shadeActive;
      case "TOGGLE_EQ":
        return eqVisible;
      case "EQ_TOGGLE":
      case "ON":
        return eq?.enabled ?? false;
      case "EQ_AUTO":
      case "AUTO":
        return eq?.auto ?? false;
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
        <SkinText
          {obj}
          {status}
          {position}
          {ticker}
          {bitrateKbps}
          {sampleRateHz}
          images={layout.images}
        />
      {:else if obj.object.kind === "status"}
        <SkinStatus {obj} images={layout.images} {status} {channels} />
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
