<script lang="ts">
  /**
   * The PLAYLIST EDITOR window (label "playlist") — a read-only view of the
   * queue snapshot with every mutation shipped to the player as a
   * queue-intent (M6b).
   *
   * Local state stays local: selection / drag-reorder interactions are this
   * window's own; the current-item highlight comes from
   * snapshot.currentIndex, and durations render from the snapshot items
   * (the player patches them when TrackStarted reports the real length).
   */
  import { open } from "@tauri-apps/plugin-dialog";

  import PlaylistEditor from "../components/PlaylistEditor.svelte";
  import { expandAudioPaths } from "../lib/library/ipc";
  import { queueIntent, queueSnapshot, windowSetSize } from "../lib/window/bus.svelte";
  import { LABEL } from "../lib/window/identity";
  import TitleStrip from "./TitleStrip.svelte";

  const snap = $derived(queueSnapshot.value);
  const items = $derived(snap?.items ?? []);
  const currentIndex = $derived(snap?.currentIndex ?? null);
  const shuffle = $derived(snap?.shuffle ?? false);
  const repeat = $derived(snap?.repeat ?? "off");

  // ADD: insert the picked files right after the current track. The indexes
  // are computed from the snapshot and sent explicitly; the player applies
  // the intents in emission order, so the block lands contiguously. (A
  // concurrent mutation elsewhere can still shift them — last writer wins.)
  async function addAfterCurrent(): Promise<void> {
    let res: string | string[] | null = null;
    try {
      res = await open({ multiple: true, directory: false });
    } catch {
      return;
    }
    const files = Array.isArray(res) ? res : res != null ? [res] : [];
    // Backend expands directories and filters to known audio extensions.
    const paths = await expandAudioPaths(files);
    if (paths.length === 0) return;
    let at = currentIndex == null ? items.length : currentIndex + 1;
    for (const path of paths) {
      queueIntent({ type: "addNext", path, index: at });
      at += 1;
    }
  }

  // ADD DIR: pick one folder; the backend walks it recursively (sorted).
  async function addDirAfterCurrent(): Promise<void> {
    let res: string | null = null;
    try {
      res = await open({ multiple: false, directory: true });
    } catch {
      return;
    }
    if (typeof res !== "string") return;
    const paths = await expandAudioPaths([res]);
    if (paths.length === 0) return;
    let at = currentIndex == null ? items.length : currentIndex + 1;
    for (const path of paths) {
      queueIntent({ type: "addNext", path, index: at });
      at += 1;
    }
  }

  // Size the OS window to the panel's natural size (initial mount + resizes,
  // e.g. the list growing) via a ResizeObserver on the root.
  let root: HTMLElement | null = $state(null);
  let lastW = 0;
  let lastH = 0;
  $effect(() => {
    const el = root;
    if (!el) return;
    const apply = () => {
      const r = el.getBoundingClientRect();
      const w = Math.ceil(r.width);
      const h = Math.ceil(r.height);
      if (w === 0 || h === 0 || (w === lastW && h === lastH)) return;
      lastW = w;
      lastH = h;
      windowSetSize(LABEL, w, h).catch(() => undefined);
    };
    apply();
    const ro = new ResizeObserver(apply);
    ro.observe(el);
    return () => ro.disconnect();
  });
</script>

<div class="win" bind:this={root}>
  <TitleStrip title="PLAYLIST EDITOR" note={`${currentIndex == null ? "--" : currentIndex + 1}/${items.length}`} />

  <div class="qbar mono">
    <span class="pos">{currentIndex == null ? "--" : currentIndex + 1}/{items.length}</span>
    <button class:on={shuffle} onclick={() => queueIntent({ type: "toggleShuffle" })}>shuffle</button>
    <button class:on={repeat !== "off"} onclick={() => queueIntent({ type: "cycleRepeat" })}>
      repeat&nbsp;{repeat}
    </button>
    <button onclick={() => queueIntent({ type: "clear" })} disabled={items.length === 0}>clear</button>
  </div>

  <PlaylistEditor
    {items}
    {currentIndex}
    onplay={(i) => queueIntent({ type: "play", index: i })}
    onremove={(i) => queueIntent({ type: "remove", index: i })}
    onmove={(from, to) => queueIntent({ type: "move", from, to })}
    onadd={() => void addAfterCurrent()}
    onadddir={() => void addDirAfterCurrent()}
  />
</div>

<style>
  .win {
    width: max-content; /* editor (275px) + padding */
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 4px;
    box-sizing: border-box;
    background: #000;
    user-select: none;
  }

  .qbar {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 275px;
    box-sizing: border-box;
    font-size: 0.75rem;
    color: #a0a0a0;
  }

  .pos {
    min-width: 7ch;
    color: #3ccb4e;
  }

  .qbar button {
    background: #1a1a1a;
    color: #c8c8c8;
    border: 1px solid #3a3a3a;
    border-radius: 3px;
    padding: 0.15rem 0.5rem;
    font-size: 0.7rem;
    letter-spacing: 0.08em;
    cursor: pointer;
  }

  .qbar button:hover:not(:disabled) {
    border-color: #3ccb4e;
    color: #3ccb4e;
  }

  .qbar button:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .qbar button.on {
    border-color: #3ccb4e;
    color: #3ccb4e;
  }

  .mono {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
  }
</style>
