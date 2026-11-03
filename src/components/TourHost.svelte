<script lang="ts">
  /**
   * Mounted once by the library window. Owns the active tour, consumes a
   * player-launched pending tour, and listens for replay requests.
   */
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import TourView from "./Tour.svelte";
  import {
    TOUR_EVENT,
    clearPendingLaunch,
    takePendingLaunch,
    type TourLaunch,
  } from "../lib/tours/launch";
  import { tourById, type Tour } from "../lib/tours/registry";
  import { completeTour, dismissTour } from "../lib/tours/state";

  interface Props {
    /** Switch the library tab to LIBRARY so tour targets are visible. */
    onensurelibrary?: () => void;
  }

  let { onensurelibrary }: Props = $props();

  let active = $state.raw<{ tour: Tour; index: number } | null>(null);

  function start(id: string, index = 0): void {
    const tour = tourById(id);
    if (!tour) return;
    onensurelibrary?.();
    clearPendingLaunch();
    active = {
      tour,
      index: Math.max(0, Math.min(index, tour.steps.length - 1)),
    };
  }

  function finish(): void {
    if (active != null) {
      completeTour(active.tour.id);
      // The welcome tour includes the AI demo and wizard; don't offer the
      // standalone AI tour again on the next update.
      if (active.tour.id === "welcome.v1") completeTour("ai-search.v1");
    }
    active = null;
  }

  function close(): void {
    if (active != null) dismissTour(active.tour.id);
    active = null;
  }

  onMount(() => {
    const pending = takePendingLaunch();
    if (pending != null) start(pending.id, pending.index ?? 0);

    const unlisten = listen<TourLaunch>(TOUR_EVENT, (event) =>
      start(event.payload.id, event.payload.index ?? 0),
    ).catch(() => () => undefined);
    return () => {
      void unlisten.then((stop) => stop());
    };
  });
</script>

{#if active}
  {#key active}
    <TourView
      tour={active.tour}
      startIndex={active.index}
      onfinished={finish}
      onclose={close} />
  {/key}
{/if}
