<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { open } from "@tauri-apps/plugin-dialog";

  import SkinSurface from "./components/skin/SkinSurface.svelte";
  import PlaylistEditor from "./components/PlaylistEditor.svelte";
  import MediaLibrary from "./components/MediaLibrary.svelte";
  import { listSkins, loadSkin } from "./lib/skin/load";
  import { mockSkinPayload } from "./lib/skin/mock";
  import { resolveLayout } from "./lib/skin/resolver";
  import type { SkinPayload } from "./lib/skin/types";
  import { queue, isAudioPath } from "./lib/player/queue.svelte";
  import type { EngineEvent, QueueItemMeta } from "./lib/player/queue.svelte";

  type AudioState = {
    status: "stopped" | "playing" | "paused";
    positionSecs: number;
    durationSecs: number | null;
    volume: number;
    underrunFrames: number;
  };

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

  // Feed errors inside the queue module surface in the same log.
  queue.setLog(logLine);

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

  // Raw engine-event text for events the queue does not claim (it returns
  // its own "queue[i] …" line for the ones it correlates).
  function rawEngineLine(e: EngineEvent): string {
    switch (e.type) {
      case "TrackStarted":
        return `▶ track #${e.id} started (${e.durationSecs?.toFixed(1) ?? "?"}s)`;
      case "TrackFinished":
        return `▪ track #${e.id} finished`;
      case "EndOfQueue":
        return "= end of queue";
      case "Error":
        return `✖ error${e.id != null ? ` (#${e.id})` : ""}: ${e.msg}`;
    }
  }

  // Engine events.
  $effect(() => {
    const unlisten = listen<EngineEvent>("audio-event", (ev) => {
      const e = ev.payload;
      const note = queue.handleEngineEvent(e);
      logLine(note ?? rawEngineLine(e));
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

  // The debug buttons below poke the engine directly (they are M1 probes of
  // the IPC surface). Detaching the queue first keeps its feed-order
  // correlation from mis-tracking a raw session's TrackStarted events.
  async function pickAndPlay() {
    const file = await open({ multiple: false, directory: false });
    if (typeof file === "string") {
      queue.onStopped();
      await cmd("audio_play", { path: file });
    }
  }

  async function pickAndEnqueue() {
    const file = await open({ multiple: false, directory: false });
    if (typeof file === "string") {
      queue.onStopped();
      await cmd("audio_enqueue", { path: file });
    }
  }

  function stopEngine() {
    queue.onStopped();
    void cmd("audio_stop");
  }

  // Multi-select audio dialog shared by EJECT and the editor's ADD.
  async function pickAudioPaths(): Promise<string[]> {
    const res = await open({ multiple: true, directory: false });
    const files = Array.isArray(res) ? res : res != null ? [res] : [];
    return files.filter(isAudioPath);
  }

  // EJECT: append to the end of the queue; if nothing was ever loaded, spin
  // up playback at the first added track.
  async function eject() {
    const paths = await pickAudioPaths();
    if (paths.length === 0) return;
    const wasUnloaded = queue.currentIndex === null;
    const first = queue.add(paths);
    if (wasUnloaded) queue.play(first);
  }

  // Playlist ADD: insert the picked files right after the current track.
  async function addAfterCurrent() {
    const paths = await pickAudioPaths();
    if (paths.length === 0) return;
    let at = queue.currentIndex == null ? queue.items.length : queue.currentIndex + 1;
    for (const path of paths) {
      queue.addNext(path, at);
      at += 1;
    }
  }

  // Media Library queue hooks: enqueue fully-tagged tracks (M4).
  function enqueueFromLibrary(metas: QueueItemMeta[]) {
    if (metas.length === 0) return;
    queue.addFromLibrary(metas);
    logLine(`+ ${metas.length} track${metas.length === 1 ? "" : "s"} enqueued from library`);
  }

  function playFromLibrary(metas: QueueItemMeta[]) {
    if (metas.length === 0) return;
    const first = queue.addFromLibrary(metas);
    queue.play(first);
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
      // Prefer a real installed skin; the built-in fallback is always listed.
      const pick = skins.find((s) => s.id !== "fallback") ?? skins[0];
      skinSource = `skin: ${pick.fileName}`;
      return await loadSkin(pick.id);
    } catch (e) {
      // No skins installed (or the scan failed) — render the mock.
      skinSource = `skin load failed (${e}; using mock)`;
      return mockSkinPayload();
    }
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

  // M6 will give the playlist editor / media library real OS windows; for
  // now the skin's TOGGLE_PLAYLIST action flips this section's visibility.
  let playlistVisible = $state(true);

  // Standard Winamp button actions → queue navigation / audio IPC.
  function skinAction(action: string) {
    switch (action) {
      case "PLAY":
        if (state.status === "paused") void cmd("audio_resume");
        else if (state.status === "stopped" && queue.items.length > 0) {
          queue.play(queue.currentIndex ?? 0);
        }
        break;
      case "PAUSE":
        if (state.status === "playing") void cmd("audio_pause");
        else if (state.status === "paused") void cmd("audio_resume");
        break;
      case "STOP":
        stopEngine();
        break;
      case "PREV":
        queue.prev();
        break;
      case "NEXT":
        queue.next();
        break;
      case "EJECT":
        void eject();
        break;
      case "TOGGLE_PLAYLIST":
        playlistVisible = !playlistVisible;
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
    <button onclick={stopEngine} disabled={state.status === "stopped"}>Stop</button>
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

  {#if playlistVisible}
    <!-- M6: becomes a real OS window (edge-snapped like the player). -->
    <section class="playlist">
      <h2>playlist editor</h2>

      <p class="mono queue-status">
        <span>{queue.currentIndex == null ? "--" : queue.currentIndex + 1}/{queue.items.length}</span>
        <button class:on={queue.shuffle} onclick={() => queue.toggleShuffle()}>shuffle</button>
        <button class:on={queue.repeat !== "off"} onclick={() => queue.cycleRepeat()}>
          repeat&nbsp;{queue.repeat}
        </button>
      </p>

      <PlaylistEditor
        items={queue.items}
        currentIndex={queue.currentIndex}
        onplay={(i) => queue.play(i)}
        onremove={(i) => queue.remove(i)}
        onmove={(from, to) => queue.move(from, to)}
        onadd={addAfterCurrent}
      />
    </section>
  {/if}

  <section class="media-library">
    <h2>media library</h2>

    <MediaLibrary
      onenqueue={enqueueFromLibrary}
      onplaynow={playFromLibrary}
      onlog={logLine} />
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

  .playlist {
    width: 100%;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.75rem;
    padding-top: 1.5rem;
    border-top: 1px solid #222;
  }

  .media-library {
    width: 100%;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.75rem;
    padding-top: 1.5rem;
    border-top: 1px solid #222;
  }

  .queue-status {
    margin: 0;
    display: flex;
    align-items: center;
    gap: 0.75rem;
    font-size: 0.75rem;
    color: #a0a0a0;
  }

  .queue-status button {
    padding: 0.15rem 0.5rem;
    font-size: 0.7rem;
    letter-spacing: 0.08em;
  }

  .skin-preview h2,
  .playlist h2,
  .media-library h2 {
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
