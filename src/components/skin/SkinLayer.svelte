<script lang="ts">
  /**
   * `<layer>`: a static image plane. `<animatedlayer>`: a horizontal
   * frame strip animated by the engine (M6).
   *
   * Frame count: the `length` param when the skin states it, otherwise the
   * strip-width / rect-width heuristic (animatedlayer strips are film strips
   * of equal-width frames, so frames = image width ÷ frame width; the
   * natural size is measured by decoding the data URL once).
   *
   * While playing, frames advance on a timer (`speed` param ms, default
   * ~125 ms) cycling within the strip; otherwise the strip parks on frame 1
   * — the classic "post-DSP" rest state (frame 0 is usually the pre-play
   * glyph).
   */
  import type { ResolvedObject } from "../../lib/skin/resolver";
  import type { PlaybackStatus } from "../../lib/skin/wasabi";

  interface Props {
    obj: ResolvedObject;
    images: Record<string, string>;
    status?: PlaybackStatus;
  }

  let { obj, images, status = "stopped" }: Props = $props();

  const isAnim = $derived(obj.object.kind === "animatedlayer");

  const o = $derived(
    obj.object.kind === "layer" || obj.object.kind === "animatedlayer" ? obj.object : null,
  );

  const url = $derived(o?.image ? images[o.image] : undefined);

  // Natural strip size for the frame-count heuristic (data URLs decode async).
  let natural = $state<{ w: number; h: number } | null>(null);

  $effect(() => {
    const u = url;
    natural = null;
    if (!u) return;
    let alive = true;
    const img = new Image();
    img.onload = () => {
      if (alive) natural = { w: img.naturalWidth, h: img.naturalHeight };
    };
    img.src = u;
    return () => {
      alive = false;
    };
  });

  const frames = $derived.by(() => {
    if (!o || !isAnim) return 1;
    const len = parseInt(o.params?.["length"] ?? "", 10);
    if (Number.isFinite(len) && len >= 1) return Math.floor(len);
    if (natural && obj.w) return Math.max(1, Math.floor(natural.w / obj.w));
    return 1;
  });

  let frame = $state(0);

  $effect(() => {
    void frames; // frame count can arrive late (image decode)
    if (!isAnim) {
      frame = 0;
      return;
    }
    if (status !== "playing") {
      frame = Math.min(1, frames - 1); // rest frame
      return;
    }
    const speed = Math.max(30, parseInt(o?.params?.["speed"] ?? "", 10) || 125);
    const timer = setInterval(() => {
      frame = (frame + 1) % frames;
    }, speed);
    return () => clearInterval(timer);
  });
</script>

{#if url}
  {#if obj.w != null && obj.h != null}
    <div
      class="img"
      style="left:{obj.x}px; top:{obj.y}px; width:{obj.w}px; height:{obj.h}px;
        background-image:url('{url}'); background-size:{frames * 100}% 100%;
        background-position:{-frame * obj.w}px 0px;"
      title={o?.id ?? undefined}
    ></div>
  {:else}
    <!-- No explicit size: fall back to the bitmap's natural size. -->
    <img class="img" src={url} style="left:{obj.x}px; top:{obj.y}px;" alt={o?.id ?? ""} />
  {/if}
{/if}

<style>
  .img {
    position: absolute;
    background-repeat: no-repeat;
  }

  div.img {
    background-size: 100% 100%;
  }

  img.img {
    display: block;
  }
</style>
