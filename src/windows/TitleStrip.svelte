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
   *
   * With `playlistTheme`, the strip is the folded playlist's current-track
   * line instead of window chrome: it uses the playlist editor's font and
   * pledit colors, matching the PE shade bar in classic Winamp.
   *
   * The `×` at the right end closes the window. It is a sibling of the strip,
   * laid over its end, because the foldable strip is a `<button>` itself and
   * a press on it would start a drag. Only the secondary windows use the
   * strip, and they only ever hide, so nothing here can quit the app.
   */
  import type { SkinTheme } from "../lib/skin/theme";
  import { windowSetVisible } from "../lib/window/bus.svelte";
  import { dragWindow } from "../lib/window/drag";
  import { LABEL } from "../lib/window/identity";

  interface Props {
    title: string;
    /** Right-aligned extra text (queue position, …). */
    note?: string;
    /** Double-click on the strip folds/unfolds the window (windowshade). */
    onfold?: () => void;
    /** Playlist shade colors; set only for the folded playlist track line. */
    playlistTheme?: SkinTheme;
    /** What the close button does; the default hides this window. */
    onclose?: () => void;
  }
  let { title, note, onfold, playlistTheme, onclose }: Props = $props();

  const playlistStyle = $derived(
    playlistTheme
      ? `--strip-fg:${playlistTheme.normal};--strip-bg:${playlistTheme.normalbg}`
      : undefined,
  );

  function close(): void {
    if (onclose) onclose();
    else windowSetVisible(LABEL, false).catch(() => undefined);
  }
</script>

{#snippet content()}
  <span class="title">{title}</span>
  {#if note}
    <span class="note mono">{note}</span>
  {/if}
{/snippet}

<div class="chrome">
  {#if onfold}
    <button
      type="button"
      class="strip foldable"
      class:playlist={playlistTheme != null}
      style={playlistStyle}
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
    <header
      class="strip"
      class:playlist={playlistTheme != null}
      style={playlistStyle}
      use:dragWindow>
      {@render content()}
    </header>
  {/if}
  <button type="button" class="x" onclick={close} title="close" aria-label={`Close ${title}`}>×</button>
</div>

<style>
  /* The strip fills this box; the close button is laid over its right end. */
  .chrome {
    position: relative;
    width: 100%;
    flex: none;
  }

  .strip {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    width: 100%;
    box-sizing: border-box;
    height: 14px;
    /* Room on the right for the close button. */
    padding: 0 24px 0 8px;
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

  .x {
    /* Sized, not stretched between top and bottom: WebKit does not stretch
       an absolutely positioned <button> (it came out 8px tall). Fits inside
       the strip's 14px border box. */
    position: absolute;
    top: 1px;
    right: 1px;
    width: 18px;
    height: 12px;
    box-sizing: border-box;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 0;
    padding: 0;
    background: transparent;
    color: #707070;
    font: 11px/1 Tahoma, "Segoe UI", Geneva, Verdana, sans-serif;
  }

  .x:hover,
  .x:focus-visible {
    color: #3ccb4e;
    outline: none;
  }

  /* Pixel font + tracked caps: the classic skinned-titlebar look. */
  .title {
    flex: 1 1 auto;
    min-width: 0;
    font-family: "Silkscreen", ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 8px; /* Silkscreen's native pixel grid */
    letter-spacing: 0.14em;
    color: #8b939c;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .note {
    flex: none;
    font-family: "Silkscreen", ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 8px;
    color: #3ccb4e;
    white-space: nowrap;
  }

  .mono {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
  }

  /* Folded playlist: the strip is a playlist row, not window chrome. Use the
     playlist editor's font and pledit colors so it matches the PE shade bar
     (green current-track line + duration) of the classic player. */
  .strip.playlist {
    background: var(--strip-bg, #000);
    color: var(--strip-fg, #00ff00);
  }

  .strip.playlist .title,
  .strip.playlist .note {
    font-family: Tahoma, "Segoe UI", Geneva, Verdana, sans-serif;
    font-size: 11px; /* same face/size as the playlist editor rows */
    line-height: 12px; /* strip content box (14px minus borders) */
    letter-spacing: 0;
    color: inherit;
  }

  .strip.playlist .title {
    flex: 1 1 auto;
    min-width: 0;
  }

  .strip.playlist .note {
    flex: none;
  }
</style>
