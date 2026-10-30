<script lang="ts">
  /**
   * Skinned-ish chrome strip for the non-skin windows (playlist / library):
   * a 14px dark titlebar that drags the OS window via `data-tauri-drag-region`
   * (the attribute must sit on the mousedown target, so every child that
   * isn't itself interactive carries it too).
   */
  interface Props {
    title: string;
    /** Right-aligned extra text (queue position, …). */
    note?: string;
  }
  let { title, note }: Props = $props();
</script>

<header class="strip" data-tauri-drag-region>
  <span class="title" data-tauri-drag-region>{title}</span>
  {#if note}
    <span class="note mono" data-tauri-drag-region>{note}</span>
  {/if}
</header>

<style>
  .strip {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    width: 100%;
    box-sizing: border-box;
    height: 14px;
    padding: 0 8px;
    background: #101010;
    border: 1px solid #2a2a2a;
    user-select: none;
    cursor: default;
  }

  /* Pixel font + tracked caps: the classic skinned-titlebar look. */
  .title {
    font-family: "Silkscreen", ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 8px; /* Silkscreen's native pixel grid */
    letter-spacing: 0.14em;
    color: #8b939c;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .note {
    font-family: "Silkscreen", ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 8px;
    color: #3ccb4e;
    white-space: nowrap;
  }

  .mono {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
  }
</style>
