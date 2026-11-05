<script lang="ts">
  /**
   * One onboarding tour. Renders a dimmed/spotlight overlay in the library
   * window and steps through the registry. Panel steps host the AI demo and
   * provider wizard; their own buttons control the advance.
   */
  import { onMount, tick } from "svelte";
  import type { Tour } from "../lib/tours/registry";
  import { registerEscape } from "../lib/window/escape";
  import AiSetupWizard from "./AiSetupWizard.svelte";
  import AiTourDemo from "./AiTourDemo.svelte";

  interface Props {
    tour: Tour;
    startIndex?: number;
    /** Last step completed. */
    onfinished: () => void;
    /** Skip/dismiss the whole tour. */
    onclose: () => void;
  }

  let { tour, startIndex = 0, onfinished, onclose }: Props = $props();

  // The TourHost remounts this component for every launch.
  // svelte-ignore state_referenced_locally
  let index = $state(
    Math.max(0, Math.min(startIndex, tour.steps.length - 1)),
  );
  let targetRect = $state<DOMRect | null>(null);
  let targetMissing = $state(false);

  const step = $derived(tour.steps[index] ?? tour.steps[0]);
  const isLast = $derived(index >= tour.steps.length - 1);
  const isPanel = $derived(step?.panel != null);

  async function measure(): Promise<void> {
    targetRect = null;
    targetMissing = false;
    if (step?.target == null) return;
    await tick();
    const element = document.querySelector(step.target);
    if (element == null) {
      targetMissing = true;
      return;
    }
    element.scrollIntoView({ block: "nearest", inline: "nearest" });
    await tick();
    targetRect = element.getBoundingClientRect();
  }

  $effect(() => {
    void index;
    void step?.target;
    void measure();
  });

  function next(): void {
    if (isLast) onfinished();
    else index += 1;
  }

  function back(): void {
    if (index > 0) index -= 1;
  }

  onMount(() => {
    const onResize = () => void measure();
    window.addEventListener("resize", onResize);
    window.addEventListener("scroll", onResize, true);
    // Escape is owned by the shared handler; leave it alone while a tour
    // input has focus so typing a prompt cannot dismiss the tour.
    const unregisterEscape = registerEscape(() => {
      const active = document.activeElement as HTMLElement | null;
      const editing =
        active?.closest("input, textarea, select, [contenteditable='true']") != null;
      if (!editing) onclose();
    });
    const onKey = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      const editing =
        target?.closest("input, textarea, select, [contenteditable='true']") != null;
      if (!editing && event.key === "ArrowRight" && !isPanel && !isLast) {
        event.preventDefault();
        next();
      } else if (!editing && event.key === "ArrowLeft" && index > 0) {
        event.preventDefault();
        back();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      unregisterEscape();
      window.removeEventListener("resize", onResize);
      window.removeEventListener("scroll", onResize, true);
      window.removeEventListener("keydown", onKey);
    };
  });
</script>

<div
  class="overlay"
  class:centered={!targetRect}
  role="dialog"
  aria-modal="true"
  aria-label={tour.title}>
  {#if targetRect}
    <!-- Four scrim rectangles leave a real, click-through hole around the
         target. A clip-path or a higher hole layer is less portable. -->
    <div class="scrim top" style="height:{Math.max(0, targetRect.top - 3)}px"></div>
    <div
      class="scrim bottom"
      style="top:{targetRect.top + targetRect.height + 3}px"></div>
    <div
      class="scrim left"
      style="top:{Math.max(0, targetRect.top - 3)}px; width:{Math.max(0, targetRect.left - 3)}px; height:{targetRect.height + 6}px"></div>
    <div
      class="scrim right"
      style="top:{Math.max(0, targetRect.top - 3)}px; left:{targetRect.left + targetRect.width + 3}px; height:{targetRect.height + 6}px"></div>
    <div
      class="hole"
      style="left:{Math.max(0, targetRect.left - 3)}px; top:{Math.max(0, targetRect.top - 3)}px; width:{targetRect.width + 6}px; height:{targetRect.height + 6}px">
    </div>
  {:else}
    <div class="scrim full"></div>
  {/if}

  <div class="card" class:centered={!targetRect} tabindex="-1">
    <div class="meta">
      <span class="badge">TOUR</span>
      <span>{tour.title}</span>
      <span class="stepno">{index + 1} / {tour.steps.length}</span>
    </div>
    <h2>{step?.title ?? tour.title}</h2>
    {#if step?.body}
      <p>{step.body}</p>
    {/if}
    {#if targetMissing}
      <div class="miss">the highlighted area isn't visible on this tab; the text still applies.</div>
    {/if}

    {#if step?.panel === "aiDemo"}
      <AiTourDemo onadvance={next} />
    {:else if step?.panel === "aiWizard"}
      <AiSetupWizard oncomplete={next} onskip={next} />
    {/if}

    <footer>
      <button class="skip" onclick={onclose}>SKIP TOUR</button>
      <span class="grow"></span>
      {#if index > 0}
        <button onclick={back}>BACK</button>
      {/if}
      {#if !isPanel}
        <button class="primary" onclick={next}>{isLast ? "DONE" : "NEXT"}</button>
      {/if}
    </footer>
  </div>
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    z-index: 80;
    display: flex;
    align-items: flex-end;
    justify-content: center;
    padding: 12px;
    box-sizing: border-box;
    pointer-events: none;
  }

  .overlay.centered {
    align-items: center;
  }

  .scrim {
    position: fixed;
    z-index: 80;
    background: rgba(0, 0, 0, 0.55);
    pointer-events: auto;
  }

  .scrim.full {
    inset: 0;
  }

  .scrim.top {
    left: 0;
    right: 0;
    top: 0;
  }

  .scrim.bottom {
    left: 0;
    right: 0;
    bottom: 0;
  }

  .scrim.left {
    left: 0;
  }

  .scrim.right {
    right: 0;
  }

  .hole {
    position: fixed;
    z-index: 81;
    border: 2px solid #3ccb4e;
    border-radius: 3px;
    pointer-events: none;
  }

  .card {
    position: relative;
    z-index: 82;
    width: min(560px, calc(100% - 8px));
    pointer-events: auto;
    max-height: calc(100% - 4px);
    overflow-y: auto;
    box-sizing: border-box;
    padding: 10px 12px;
    border: 1px solid #3a3a3a;
    background: #161616;
    color: #c8c8c8;
    font-family: "Segoe UI", "Helvetica Neue", Helvetica, Arial, sans-serif;
    user-select: none;
    scrollbar-width: thin;
    scrollbar-color: #2f4f33 #000;
  }

  .card:focus {
    outline: none;
  }

  .meta {
    display: flex;
    align-items: center;
    gap: 6px;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 9px;
    color: #707070;
  }

  .badge {
    padding: 0 4px;
    background: #3ccb4e;
    color: #0d0d0d;
    border-radius: 2px;
    font-weight: 600;
  }

  .stepno {
    margin-left: auto;
  }

  h2 {
    margin: 6px 0 3px;
    font-size: 14px;
    font-weight: 600;
    letter-spacing: 0.02em;
    color: #e0e0e6;
  }

  p {
    margin: 0 0 6px;
    font-size: 11px;
    line-height: 1.45;
    color: #b8b8c0;
  }

  .miss {
    margin: 0 0 6px;
    font-size: 10px;
    color: #d8a63c;
  }

  footer {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 8px;
    padding-top: 6px;
    border-top: 1px solid #262626;
  }

  .grow {
    flex: 1;
  }

  button {
    background: #1c1c1c;
    color: #9a9a9a;
    border: 1px solid #3a3a3a;
    border-radius: 2px;
    padding: 3px 9px;
    font-size: 10px;
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

  button.skip {
    border-color: transparent;
    background: transparent;
    color: #707070;
  }
</style>
