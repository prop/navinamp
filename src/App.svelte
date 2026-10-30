<script lang="ts">
  /**
   * App is now a ROUTER on the window label (M6b): the Tauri config creates
   * four OS windows — "main" (player: owns queue/playback state), and the
   * bus-driven "playlist" / "library" / "eq" views. An unknown label
   * degrades to the player view (lib/window/identity normalizes it before
   * we get here).
   *
   * initWindowBus() must run before any view mounts: it wires the
   * windows-changed listener in every window, the player-only listeners
   * (queue-intent executor, queue-state broadcast, media-command) and the
   * non-player listeners (queue-state consumer + mount-time snapshot
   * request). See lib/window/bus.svelte.ts for the protocol.
   */
  import EqWindow from "./windows/EqWindow.svelte";
  import LibraryWindow from "./windows/LibraryWindow.svelte";
  import PlayerWindow from "./windows/PlayerWindow.svelte";
  import PlaylistWindow from "./windows/PlaylistWindow.svelte";
  import { initWindowBus } from "./lib/window/bus.svelte";
  import { LABEL } from "./lib/window/identity";

  initWindowBus();
</script>

{#if LABEL === "playlist"}
  <PlaylistWindow />
{:else if LABEL === "library"}
  <LibraryWindow />
{:else if LABEL === "eq"}
  <EqWindow />
{:else}
  <PlayerWindow />
{/if}

<style>
  /* Native-app feel: arrow cursor everywhere (never the webby I-beam or
     hand), no text selection — EXCEPT real inputs, which keep their text
     cursor and selectability. !important beats component-scoped rules. */
  :global(*) {
    cursor: default !important;
    user-select: none !important;
    -webkit-user-select: none !important;
  }

  :global(input),
  :global(textarea) {
    cursor: text !important;
    user-select: text !important;
    -webkit-user-select: text !important;
  }

  /* Bundled OFL pixel font (Jason Kottke's Silkscreen) — used by the skin
     text renderer and window chrome in every window; declared here so the
     face exists document-wide regardless of which components mount.
     License: src/assets/fonts/silkscreen-OFL.txt. */
  @font-face {
    font-family: "Silkscreen";
    font-style: normal;
    font-weight: 400;
    font-display: swap;
    src: url("./assets/fonts/silkscreen-latin.woff2") format("woff2");
  }

  :global(html),
  :global(body) {
    margin: 0;
    padding: 0;
    background: #000;
    overflow: hidden;
  }
</style>
