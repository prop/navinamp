<script lang="ts">
  /**
   * App is now a ROUTER on the window label (M6b): the Tauri config creates
   * three OS windows — "main" (player: owns queue/playback state), and the
   * bus-driven "playlist" / "library" views. An unknown label degrades to
   * the player view (lib/window/identity normalizes it before we get here).
   *
   * initWindowBus() must run before any view mounts: it wires the
   * windows-changed listener in every window, the player-only listeners
   * (queue-intent executor, queue-state broadcast, media-command) and the
   * non-player listeners (queue-state consumer + mount-time snapshot
   * request). See lib/window/bus.svelte.ts for the protocol.
   */
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
{:else}
  <PlayerWindow />
{/if}

<style>
  :global(html),
  :global(body) {
    margin: 0;
    padding: 0;
    background: #000;
    overflow: hidden;
  }
</style>
