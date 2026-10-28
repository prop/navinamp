<script lang="ts">
  /**
   * `<text>`: static text for now. The songticker (`display="songticker"`)
   * shows a placeholder until the queue exposes track metadata.
   * TODO(songticker): scrolling marquee + live track title/artist.
   */
  import type { ResolvedObject } from "../../lib/skin/resolver";

  interface Props {
    obj: ResolvedObject;
  }

  let { obj }: Props = $props();

  const o = $derived(obj.object.kind === "text" ? obj.object : null);

  const text = $derived.by(() => {
    if (!o) return "";
    if (o.params?.["text"]) return o.params["text"];
    if (o.display === "songticker") return "NavAmp — skin preview";
    return o.id ?? "";
  });
</script>

{#if o}
  <div
    class="txt"
    style="left:{obj.x}px; top:{obj.y}px; width:{obj.w ?? 0}px; height:{obj.h ?? 0}px;{o.params?.[
      'color'
    ]
      ? `color:${o.params['color']};`
      : ''}{o.params?.['font'] ? `font-family:${o.params['font']};` : ''}{o.params?.['fontsize']
      ? `font-size:${o.params['fontsize']}px;`
      : ''}{o.params?.['bold'] === '1' ? 'font-weight:bold;' : ''}"
  >{text}</div>
{/if}

<style>
  .txt {
    position: absolute;
    overflow: hidden;
    white-space: nowrap;
    display: flex;
    align-items: center;
    color: #3ccb4e;
    font-family: Verdana, Tahoma, sans-serif;
    font-size: 9px;
    line-height: 1;
    user-select: none;
  }
</style>
