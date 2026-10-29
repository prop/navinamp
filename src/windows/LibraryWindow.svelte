<script lang="ts">
  /**
   * The LIBRARY window (label "library") — everything that used to live in
   * the stacked M1-M5 app: Media Library (with its Navidrome settings +
   * smart-view modals), the EQ panel, and the M1 debug surface as a
   * collapsible DEV section (M6b).
   *
   * Queue hooks are intents: ENQUEUE/PLAY append via the player; the queue
   * status line in the title strip renders from snapshots.
   *
   * The DEV block keeps the M1 engine probes: transport buttons go through
   * `media-command` (relayed to the player, whose executor applies the same
   * semantics as the skin buttons — a raw `audio_stop` from here would
   * desync the player's feed-order correlation), while the file dialogs and
   * seek/volume sliders stay direct IPC like before.
   */
  import { invoke } from "@tauri-apps/api/core";
  import { open } from "@tauri-apps/plugin-dialog";

  import EqPanel from "../components/EqPanel.svelte";
  import MediaLibrary from "../components/MediaLibrary.svelte";
  import {
    emitMediaCommand,
    queueIntent,
    queuePlayNow,
    queueSnapshot,
    windowSetSize,
  } from "../lib/window/bus.svelte";
  import { LABEL } from "../lib/window/identity";
  import TitleStrip from "./TitleStrip.svelte";

  const snap = $derived(queueSnapshot.value);
  const queueNote = $derived(
    `QUEUE ${snap?.currentIndex == null ? "--" : snap.currentIndex + 1}/${snap?.items.length ?? 0}`,
  );

  let log = $state<string[]>([]);

  function logLine(line: string) {
    log = [`${new Date().toLocaleTimeString()}  ${line}`, ...log].slice(0, 8);
  }

  // ── DEV block (M1 debug surface) ─────────────────────────────────────────

  let devOpen = $state(false);

  type AudioState = {
    status: "stopped" | "playing" | "paused";
    positionSecs: number;
    durationSecs: number | null;
    volume: number;
    underrunFrames: number;
  };
  let devState = $state<AudioState>({
    status: "stopped",
    positionSecs: 0,
    durationSecs: null,
    volume: 1,
    underrunFrames: 0,
  });
  let seeking = $state<number | null>(null);

  $effect(() => {
    const timer = setInterval(async () => {
      try {
        devState = await invoke<AudioState>("audio_state");
      } catch {
        // backend without audio_state — keep the last state
      }
    }, 100);
    return () => clearInterval(timer);
  });

  async function devCmd(name: string, args?: object): Promise<void> {
    try {
      await invoke(name, args);
    } catch (e) {
      logLine(`${name} error: ${e}`);
    }
  }

  // Raw file probes (M1): detach the player's queue correlation first (the
  // onStopped intent), then poke the engine directly. The relay makes
  // ordering best-effort — acceptable for a debug surface, not the product path.
  async function devOpenPlay(): Promise<void> {
    const file = await open({ multiple: false, directory: false });
    if (typeof file === "string") {
      queueIntent({ type: "onStopped" });
      await devCmd("audio_play", { path: file });
    }
  }

  async function devEnqueue(): Promise<void> {
    const file = await open({ multiple: false, directory: false });
    if (typeof file === "string") {
      queueIntent({ type: "onStopped" });
      await devCmd("audio_enqueue", { path: file });
    }
  }

  const fmt = (secs: number) => {
    const m = Math.floor(secs / 60);
    const s = Math.floor(secs % 60);
    return `${m}:${String(s).padStart(2, "0")}`;
  };

  const duration = $derived(devState.durationSecs ?? 0);
  const position = $derived(
    Math.min(seeking ?? devState.positionSecs, duration || Infinity),
  );

  // ── window sizing: natural panel size (mount + resizes) ─────────────────

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
  <TitleStrip title="MEDIA LIBRARY / EQ / DEV" note={queueNote} />

  <MediaLibrary
    onenqueue={(metas) => queueIntent({ type: "addFromLibrary", metas })}
    onplaynow={(metas) => queuePlayNow(metas)}
    onlog={logLine} />

  <EqPanel />

  <section class="dev">
    <button class="devtoggle" class:on={devOpen} onclick={() => (devOpen = !devOpen)}>
      DEV {devOpen ? "▾" : "▸"}
    </button>

    {#if devOpen}
      <div class="devbox">
        <div class="row">
          <button onclick={() => void devOpenPlay()}>Open &amp; Play…</button>
          <button onclick={() => void devEnqueue()}>Enqueue…</button>
          <button onclick={() => emitMediaCommand("pause")} disabled={devState.status !== "playing"}>
            Pause
          </button>
          <button onclick={() => emitMediaCommand("play")} disabled={devState.status !== "paused"}>
            Resume
          </button>
          <button onclick={() => emitMediaCommand("stop")} disabled={devState.status === "stopped"}>
            Stop
          </button>
        </div>

        <div class="row">
          <label>
            seek
            <input
              type="range"
              min="0"
              max={Math.max(duration, 0.1)}
              step="0.1"
              value={position}
              disabled={devState.status === "stopped" || duration === 0}
              oninput={(e) => (seeking = Number(e.currentTarget.value))}
              onchange={(e) => {
                seeking = null;
                void devCmd("audio_seek", { secs: Number(e.currentTarget.value) });
              }}
            />
          </label>
          <span class="mono">{fmt(position)} / {duration ? fmt(duration) : "--:--"}</span>
        </div>

        <div class="row">
          <label>
            vol
            <input
              type="range"
              min="0"
              max="1"
              step="0.01"
              value={devState.volume}
              oninput={(e) => void devCmd("audio_set_volume", { volume: Number(e.currentTarget.value) })}
            />
          </label>
          <span class="mono">{Math.round(devState.volume * 100)}%</span>
        </div>

        <p class="status {devState.status}">
          {devState.status.toUpperCase()}
          {#if devState.underrunFrames > 0}
            <span class="warn">(underruns: {devState.underrunFrames})</span>
          {/if}
        </p>

        {#if log.length > 0}
          <ul class="log">
            {#each log as line}
              <li>{line}</li>
            {/each}
          </ul>
        {/if}
      </div>
    {/if}
  </section>
</div>

<style>
  .win {
    width: max-content; /* panel (640px) + padding */
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    padding: 6px;
    background: #000;
    color: #c8c8c8;
    font-family: "Segoe UI", "Helvetica Neue", Helvetica, Arial, sans-serif;
    user-select: none;
  }

  .dev {
    width: 100%;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.5rem;
    padding-top: 0.75rem;
    border-top: 1px solid #222;
  }

  .devtoggle {
    background: #1a1a1a;
    color: #707070;
    border: 1px solid #3a3a3a;
    border-radius: 3px;
    padding: 0.15rem 0.8rem;
    font-size: 0.7rem;
    letter-spacing: 0.12em;
    cursor: pointer;
  }

  .devtoggle:hover,
  .devtoggle.on {
    border-color: #3ccb4e;
    color: #3ccb4e;
  }

  .devbox {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.6rem;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .devbox button {
    background: #1a1a1a;
    color: #c8c8c8;
    border: 1px solid #3a3a3a;
    border-radius: 3px;
    padding: 0.35rem 0.8rem;
    font-size: 0.85rem;
    cursor: pointer;
  }

  .devbox button:hover:not(:disabled) {
    border-color: #3ccb4e;
    color: #3ccb4e;
  }

  .devbox button:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .devbox label {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.8rem;
    color: #808080;
  }

  .devbox input[type="range"] {
    width: 260px;
    accent-color: #3ccb4e;
  }

  .mono {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 0.85rem;
    color: #a0a0a0;
    min-width: 9ch;
  }

  .status {
    margin: 0;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 0.9rem;
    letter-spacing: 0.15em;
  }

  .status.playing {
    color: #3ccb4e;
  }

  .status.paused {
    color: #d8a63c;
  }

  .status.stopped {
    color: #808080;
  }

  .warn {
    color: #d8604c;
  }

  .log {
    margin: 0;
    padding: 0.5rem 1rem;
    list-style: none;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 0.75rem;
    color: #707070;
    text-align: left;
    max-width: 80%;
  }

  .log li {
    white-space: nowrap;
  }
</style>
