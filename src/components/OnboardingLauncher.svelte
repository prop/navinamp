<script lang="ts">
  /**
   * First-run and What's-new prompt in the player window. The full tour lives
   * in the library window (which this opens); the player only offers a compact
   * entry card so it never blocks the tiny skinned surface.
   */
  import { getVersion } from "@tauri-apps/api/app";
  import type { Tour } from "../lib/tours/registry";
  import { launchTour } from "../lib/tours/launch";
  import {
    dismissTour,
    markVersionSeen,
    pendingUpdateTours,
    shouldShowWelcome,
  } from "../lib/tours/state";

  type Mode = "welcome" | "whatsnew" | null;

  let mode = $state<Mode>(null);
  let version = $state("");
  let pending = $state.raw<Tour[]>([]);

  $effect(() => {
    void (async () => {
      let current = "";
      try {
        current = await getVersion();
      } catch {
        // browser preview / no Tauri: still allow first-run tour
      }
      version = current || "0.0.0";

      if (shouldShowWelcome()) {
        mode = "welcome";
        return;
      }
      const updates = pendingUpdateTours(version).filter(
        (tour) => tour.id !== "welcome.v1",
      );
      if (updates.length > 0) {
        pending = updates;
        mode = "whatsnew";
      }
    })();
  });

  async function startWelcome(): Promise<void> {
    const shown = await launchTour("welcome.v1", 1);
    if (shown) markVersionSeen(version);
    mode = null;
  }

  function skipWelcome(): void {
    markVersionSeen(version);
    dismissTour("welcome.v1");
    mode = null;
  }

  async function startUpdate(): Promise<void> {
    const tour = pending[0];
    if (tour == null) return;
    const shown = await launchTour(tour.id, 0);
    // With more than one pending tour, keep seenVersion unchanged so the next
    // one can be offered after this one is completed or dismissed.
    if (shown && pending.length === 1) markVersionSeen(version);
    mode = null;
  }

  function dismissUpdate(): void {
    markVersionSeen(version);
    mode = null;
  }
</script>

{#if mode != null}
  <div class="onboard" class:pill={mode === "whatsnew"}>
    {#if mode === "welcome"}
      <div class="head">
        <strong>Welcome to NaVinaMP</strong>
        <button class="x" onclick={skipWelcome} title="skip the tour">×</button>
      </div>
      <p>Take a one-minute tour of the library and AI search — no account needed.</p>
      <div class="actions">
        <button class="primary" onclick={startWelcome}>START TOUR</button>
        <button onclick={skipWelcome}>SKIP</button>
      </div>
    {:else}
      <span class="star">★</span>
      <button class="what" onclick={startUpdate} title="what's new in this version">
        WHAT'S NEW{pending.length === 1 ? `: ${pending[0].title.toUpperCase()}` : ""}
      </button>
      <button class="x" onclick={dismissUpdate} title="dismiss">×</button>
    {/if}
  </div>
{/if}

<style>
  .onboard {
    position: absolute;
    left: 6px;
    right: 6px;
    bottom: 6px;
    z-index: 40;
    box-sizing: border-box;
    padding: 6px 7px;
    border: 1px solid #3a3a3a;
    background: #161616;
    color: #c8c8c8;
    font-family: "Segoe UI", "Helvetica Neue", Helvetica, Arial, sans-serif;
    font-size: 10px;
    box-shadow: 0 3px 12px rgba(0, 0, 0, 0.65);
    user-select: none;
  }

  .onboard.pill {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 3px 5px;
    border-color: #2f4f33;
    background: #0d0d0d;
  }

  .head {
    display: flex;
    align-items: center;
    gap: 5px;
  }

  .head strong {
    flex: 1;
    color: #e0e0e6;
    font-size: 11px;
    letter-spacing: 0.02em;
  }

  p {
    margin: 3px 0 5px;
    line-height: 1.4;
    color: #a8a8b0;
  }

  .actions {
    display: flex;
    gap: 5px;
  }

  .star {
    color: #d8a63c;
    font-size: 11px;
  }

  .what {
    flex: 1;
    text-align: left;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    color: #3ccb4e;
  }

  .x {
    flex: none;
    padding: 0 4px;
    border-color: transparent;
    background: transparent;
    color: #707070;
  }

  button {
    background: #1c1c1c;
    color: #9a9a9a;
    border: 1px solid #3a3a3a;
    border-radius: 2px;
    padding: 2px 7px;
    font-size: 9px;
    letter-spacing: 0.06em;
    cursor: pointer;
  }

  button:hover {
    border-color: #3ccb4e;
    color: #3ccb4e;
  }

  button.primary {
    border-color: #3ccb4e;
    color: #3ccb4e;
  }
</style>
