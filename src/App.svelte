<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { open } from "@tauri-apps/plugin-dialog";

  type AudioState = {
    status: "stopped" | "playing" | "paused";
    positionSecs: number;
    durationSecs: number | null;
    volume: number;
    underrunFrames: number;
  };

  type EngineEvent =
    | { type: "TrackStarted"; id: number; durationSecs: number | null }
    | { type: "TrackFinished"; id: number }
    | { type: "EndOfQueue" }
    | { type: "Error"; id: number | null; msg: string };

  let state = $state<AudioState>({
    status: "stopped",
    positionSecs: 0,
    durationSecs: null,
    volume: 1,
    underrunFrames: 0,
  });
  let log = $state<string[]>([]);
  let seeking = $state<number | null>(null);

  function logLine(line: string) {
    log = [`${new Date().toLocaleTimeString()}  ${line}`, ...log].slice(0, 8);
  }

  async function refresh() {
    try {
      state = await invoke<AudioState>("audio_state");
    } catch (e) {
      logLine(`state error: ${e}`);
    }
  }

  // Poll engine state at 10 Hz.
  $effect(() => {
    const timer = setInterval(refresh, 100);
    return () => clearInterval(timer);
  });

  // Engine events.
  $effect(() => {
    const unlisten = listen<EngineEvent>("audio-event", (ev) => {
      const e = ev.payload;
      switch (e.type) {
        case "TrackStarted":
          logLine(`▶ track #${e.id} started (${e.durationSecs?.toFixed(1) ?? "?"}s)`);
          break;
        case "TrackFinished":
          logLine(`▪ track #${e.id} finished`);
          break;
        case "EndOfQueue":
          logLine("= end of queue");
          break;
        case "Error":
          logLine(`✖ error${e.id != null ? ` (#${e.id})` : ""}: ${e.msg}`);
          break;
      }
      void refresh();
    });
    return () => {
      void unlisten.then((f) => f());
    };
  });

  async function cmd(name: string, ...args: unknown[]) {
    try {
      await invoke(name, args.length > 0 ? (args[0] as object) : undefined);
    } catch (e) {
      logLine(`${name} error: ${e}`);
    }
    void refresh();
  }

  async function pickAndPlay() {
    const file = await open({ multiple: false, directory: false });
    if (typeof file === "string") {
      await cmd("audio_play", { path: file });
    }
  }

  async function pickAndEnqueue() {
    const file = await open({ multiple: false, directory: false });
    if (typeof file === "string") {
      await cmd("audio_enqueue", { path: file });
    }
  }

  const fmt = (secs: number) => {
    const m = Math.floor(secs / 60);
    const s = Math.floor(secs % 60);
    return `${m}:${String(s).padStart(2, "0")}`;
  };

  const duration = $derived(state.durationSecs ?? 0);
  const position = $derived(
    Math.min(seeking ?? state.positionSecs, duration || Infinity),
  );
</script>

<main>
  <h1>NavAmp M1 — debug UI</h1>

  <div class="row">
    <button onclick={pickAndPlay}>Open &amp; Play…</button>
    <button onclick={pickAndEnqueue}>Enqueue…</button>
    <button onclick={() => cmd("audio_pause")} disabled={state.status !== "playing"}>Pause</button>
    <button onclick={() => cmd("audio_resume")} disabled={state.status !== "paused"}>Resume</button>
    <button onclick={() => cmd("audio_stop")} disabled={state.status === "stopped"}>Stop</button>
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
        disabled={state.status === "stopped" || duration === 0}
        oninput={(e) => (seeking = Number(e.currentTarget.value))}
        onchange={(e) => {
          seeking = null;
          void cmd("audio_seek", { secs: Number(e.currentTarget.value) });
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
        value={state.volume}
        oninput={(e) => cmd("audio_set_volume", { volume: Number(e.currentTarget.value) })}
      />
    </label>
    <span class="mono">{Math.round(state.volume * 100)}%</span>
  </div>

  <p class="status {state.status}">
    {state.status.toUpperCase()}
    {#if state.underrunFrames > 0}
      <span class="warn">(underruns: {state.underrunFrames})</span>
    {/if}
  </p>

  {#if log.length > 0}
    <ul class="log">
      {#each log as line}
        <li>{line}</li>
      {/each}
    </ul>
  {/if}
</main>

<style>
  :global(html),
  :global(body) {
    margin: 0;
    padding: 0;
    background: #000;
  }

  main {
    min-height: 100vh;
    box-sizing: border-box;
    padding: 2rem;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 1rem;
    background: #000;
    color: #c8c8c8;
    font-family: "Segoe UI", "Helvetica Neue", Helvetica, Arial, sans-serif;
    user-select: none;
  }

  h1 {
    margin: 0;
    font-size: 1.4rem;
    font-weight: 600;
    letter-spacing: 0.04em;
    color: #3ccb4e; /* classic Winamp green */
  }

  .row {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  button {
    background: #1a1a1a;
    color: #c8c8c8;
    border: 1px solid #3a3a3a;
    border-radius: 3px;
    padding: 0.35rem 0.8rem;
    font-size: 0.85rem;
    cursor: pointer;
  }

  button:hover:not(:disabled) {
    border-color: #3ccb4e;
    color: #3ccb4e;
  }

  button:disabled {
    opacity: 0.4;
    cursor: default;
  }

  label {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.8rem;
    color: #808080;
  }

  input[type="range"] {
    width: 260px;
    accent-color: #3ccb4e;
  }

  .mono,
  .status {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
  }

  .mono {
    font-size: 0.85rem;
    color: #a0a0a0;
    min-width: 9ch;
  }

  .status {
    margin: 0;
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
