<script lang="ts">
  /**
   * Skinned-ish chrome strip for the non-skin windows (playlist / library):
   * a 14px dark titlebar that drags the OS window after a small pointer
   * movement (lib/window/drag — a Tauri drag region would swallow the
   * double-click and maximize instead).
   *
   * With `onfold`, the strip is also the windowshade control: double-click
   * toggles it, and as a real `<button>` it is keyboard reachable (Enter /
   * Space produce a detail-0 click; ordinary mouse clicks are ignored).
   */
  import { dragWindow } from "../lib/window/drag";

  interface Props {
    title: string;
    /** Right-aligned extra text (queue position, …). */
    note?: string;
    /** Double-click on the strip folds/unfolds the window (windowshade). */
    onfold?: () => void;
  }
  let { title, note, onfold }: Props = $props();
</script>

{#snippet content()}
  <span class="title">{title}</span>
  {#if note}
    <span class="note mono">{note}</span>
  {/if}
{/snippet}

{#if onfold}
  <button
    type="button"
    class="strip foldable"
    use:dragWindow
    aria-label={`${title} — press Enter to fold or unfold`}
    title="Drag to move — double-click folds the window"
    ondblclick={onfold}
    onclick={(e) => {
      // detail 0 = keyboard activation (Enter/Space); mouse clicks wait for
      // the classic double-click.
      if (e.detail === 0) onfold?.();
    }}>
    {@render content()}
  </button>
{:else}
  <header class="strip" use:dragWindow>
    {@render content()}
  </header>
{/if}

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

  /* Button reset for the foldable variant (it is a real <button>). */
  .strip.foldable {
    appearance: none;
    margin: 0;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
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
