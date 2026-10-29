<script lang="ts">
  /**
   * Placeholders for objects the renderer does not paint yet: `<status>` and
   * `<grid>`. (`<vis>` got a real canvas component in M6 — SkinVis.) Labelled
   * dashed boxes so they stay visible (and debuggable) in the layout.
   */
  import type { ResolvedObject } from "../../lib/skin/resolver";

  interface Props {
    obj: ResolvedObject;
  }

  let { obj }: Props = $props();

  const o = $derived(
    obj.object.kind === "status" || obj.object.kind === "grid" ? obj.object : null,
  );
</script>

{#if o}
  <div
    class="ph"
    style="left:{obj.x}px; top:{obj.y}px; width:{obj.w ?? 40}px; height:{obj.h ?? 16}px;"
    title="{obj.object.kind} placeholder"
  >{o.id ?? obj.object.kind}</div>
{/if}

<style>
  .ph {
    position: absolute;
    display: flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
    border: 1px dashed rgba(60, 203, 78, 0.55);
    background: rgba(0, 0, 0, 0.25);
    color: rgba(60, 203, 78, 0.75);
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 8px;
    letter-spacing: 0.08em;
    white-space: nowrap;
    pointer-events: none;
    user-select: none;
  }
</style>
