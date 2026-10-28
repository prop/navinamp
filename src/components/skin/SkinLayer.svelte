<script lang="ts">
  /**
   * `<layer>` and `<animatedlayer>`: a static image plane.
   *
   * animatedlayer TODO: the image is a horizontal frame strip; for now only
   * the first frame is shown (`background-size` frames×100% wide). Frame
   * advance from params (`length`/`framewidth`/`speed`) comes with the real
   * skin engine + audio hooks.
   */
  import type { ResolvedObject } from "../../lib/skin/resolver";

  interface Props {
    obj: ResolvedObject;
    images: Record<string, string>;
  }

  let { obj, images }: Props = $props();

  const o = $derived(
    obj.object.kind === "layer" || obj.object.kind === "animatedlayer" ? obj.object : null,
  );

  const url = $derived(o?.image ? images[o.image] : undefined);

  // Number of frames in the strip (1 for plain layers) — sizes the background
  // so exactly one frame is visible.
  const frames = $derived(
    Math.max(1, parseInt(o?.params?.["length"] ?? "1", 10) || 1),
  );
</script>

{#if url}
  {#if obj.w != null && obj.h != null}
    <div
      class="img"
      style="left:{obj.x}px; top:{obj.y}px; width:{obj.w}px; height:{obj.h}px;
        background-image:url('{url}'); background-size:{frames * 100}% 100%;"
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
