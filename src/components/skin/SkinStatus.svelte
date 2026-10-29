<script lang="ts">
  /**
   * `<status>`: image-backed status indicators (classic `.wsz` synthesis).
   * Two families, picked by `params["display"]`:
   *
   *  · play/pause/stop indicator (no `display`, the classic
   *    playpaus.bmp object): the image follows the playback status —
   *    `params["playimage"]` / `pauseimage` / `stopimage` (stopped shows
   *    the stop glyph, like Winamp and Webamp both do).
   *  · mono/stereo channel indicators (`display="mono"` / `"stereo"`, the
   *    monoster.bmp objects): two separate labels. The LIT state
   *    (`params["activeimage"]`, monoster.bmp's top row) needs the audio
   *    channel count, which the engine does not report yet — so the DIM
   *    state (`params["image"]`, the bottom row) is rendered until
   *    `audio_state` carries channels. TODO(engine): flip to activeimage
   *    when channels === 1 (mono) / 2 (stereo).
   *
   * Unknown status objects without any image params render nothing (no
   * placeholder boxes — they are what the acceptance run's "z.mon.ster"
   * garbage came from).
   */
  import type { ResolvedObject } from "../../lib/skin/resolver";
  import type { PlaybackStatus } from "../../lib/skin/wasabi";

  interface Props {
    obj: ResolvedObject;
    images: Record<string, string>;
    status: PlaybackStatus;
  }

  let { obj, images, status }: Props = $props();

  const o = $derived(obj.object.kind === "status" ? obj.object : null);
  const params = $derived(o?.params ?? {});
  const display = $derived(params["display"]);

  const label = $derived.by(() => {
    switch (display) {
      case "mono":
        return "mono (dim until channel info)";
      case "stereo":
        return "stereo (dim until channel info)";
      default:
        return "playback status";
    }
  });

  // The image to paint: dim mono/stereo cell, or the play-status glyph.
  const url = $derived.by(() => {
    if (!o) return undefined;
    switch (display) {
      case "mono":
      case "stereo":
        return params["image"] ? images[params["image"]] : undefined;
      default: {
        const key =
          status === "playing"
            ? "playimage"
            : status === "paused"
              ? "pauseimage"
              : "stopimage";
        return params[key] ? images[params[key]] : undefined;
      }
    }
  });
</script>

{#if o && url}
  <img
    class="st"
    src={url}
    style="left:{obj.x}px; top:{obj.y}px; width:{obj.w ?? 9}px; height:{obj.h ?? 9}px;"
    alt=""
    aria-hidden="true"
    title={label}
    draggable="false"
  />
{/if}

<style>
  .st {
    position: absolute;
    image-rendering: pixelated;
    pointer-events: none;
    user-select: none;
  }
</style>
