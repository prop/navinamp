<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { open } from "@tauri-apps/plugin-dialog";

  import SkinSurface from "./components/skin/SkinSurface.svelte";
  import { listSkins, loadSkin } from "./lib/skin/load";
  import { mockSkinPayload } from "./lib/skin/mock";
  import { resolveLayout } from "./lib/skin/resolver";
  import type { SkinPayload } from "./lib/skin/types";

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

  // --- skin preview: first installed .wal, else the built-in mock -----------

  let skin = $state<SkinPayload | null>(null);
  let skinError = $state<string | null>(null);
  let skinSource = $state("");
  let scale = $state(1);

  async function loadInitialSkin(): Promise<SkinPayload> {
    try {
      const skins = await listSkins();
      if (skins.length > 0) {
        skinSource = `skin: ${skins[0].fileName}`;
        return await loadSkin(skins[0].id);
      }
    } catch (e) {
      // No skins installed (or the scan failed) — render the mock.
      skinSource = `no installed skins (${e}; using mock)`;
      return mockSkinPayload();
    }
    skinSource = "mock (no installed skins)";
    return mockSkinPayload();
  }

  $effect(() => {
    let cancelled = false;
    loadInitialSkin()
      .then((payload) => {
        if (!cancelled) skin = payload;
      })
      .catch((e) => {
        if (!cancelled) skinError = String(e);
      });
    return () => {
      cancelled = true;
    };
  });

  const playerLayout = $derived(skin ? resolveLayout(skin, "main", "normal") : null);

  // Standard Winamp button actions → audio IPC. PREV/NEXT need queue
  // navigation (engine-side) and EJECT a file-open dialog — TODO until then.
  function skinAction(action: string) {
    switch (action) {
      case "PLAY":
        // TODO: with nothing loaded there is nothing to start; a real PLAY
        // should (re)start the current track once a queue exists.
        if (state.status === "paused") void cmd("audio_resume");
        break;
      case "PAUSE":
        if (state.status === "playing") void cmd("audio_pause");
        else if (state.status === "paused") void cmd("audio_resume");
        break;
      case "STOP":
        void cmd("audio_stop");
        break;
      case "PREV":
      case "NEXT":
      case "EJECT":
        // TODO: queue navigation / open-file dialog.
        break;
    }
  }
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

  <section class="skin-preview">
    <h2>
      skin preview
      <span class="skin-name">{skin?.skin.params?.["name"] ?? "…"}</span>
      <span class="skin-src">{skinSource}</span>
    </h2>

    <div class="row">
      <button class:on={scale === 1} onclick={() => (scale = 1)}>1×</button>
      <button class:on={scale === 2} onclick={() => (scale = 2)}>2×</button>
      {#if (skin?.warnings.length ?? 0) + (playerLayout?.warnings.length ?? 0) > 0}
        <span class="mono warn">
          ⚠ {(skin?.warnings.length ?? 0) + (playerLayout?.warnings.length ?? 0)} warnings
        </span>
      {/if}
    </div>

    {#if playerLayout}
      <SkinSurface
        layout={playerLayout}
        {scale}
        {position}
        {duration}
        volume={state.volume}
        onaction={skinAction}
        onseek={(secs) => cmd("audio_seek", { secs })}
        onvolume={(v) => cmd("audio_set_volume", { volume: v })}
      />
    {:else if skinError}
      <p class="status warn">skin load failed: {skinError}</p>
    {:else}
      <p class="status">loading skin…</p>
    {/if}
  </section>
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

  .skin-preview {
    width: 100%;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.75rem;
    padding-top: 1.5rem;
    border-top: 1px solid #222;
  }

  .skin-preview h2 {
    margin: 0;
    font-size: 0.8rem;
    font-weight: 600;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: #707070;
  }

  .skin-name {
    font-weight: 400;
    text-transform: none;
    letter-spacing: 0.02em;
    color: #4f4f4f;
  }

  .skin-src {
    font-weight: 400;
    text-transform: none;
    letter-spacing: 0.02em;
    color: #3a5a3f;
  }

  button.on {
    border-color: #3ccb4e;
    color: #3ccb4e;
  }
</style>
