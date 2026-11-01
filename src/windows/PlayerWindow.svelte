<script lang="ts">
  /**
   * The PLAYER window (label "main") — the skin surface, and the single
   * owner of playback state (M6b).
   *
   * Everything stateful lives HERE and only here: the queue module, the
   * 10 Hz `audio_state` poll, the engine-event listener (queue feed-order
   * correlation), the spectrum poller (via SkinVis) and the OS
   * media-metadata push. The other windows are bus-driven views.
   *
   * Layout: the resolved layout follows the window manager's layout mode
   * (normal / shade; a skin without a shade layout degrades to normal via
   * the resolver), and the OS window is sized to the layout rect × scale —
   * which is also how a shade toggle resizes the window.
   *
   * Dragging: the skin draws its own titlebar, so a transparent
   * `data-tauri-drag-region` layer overlays the titlebar band discovered
   * from the skin (lib/skin/titlebar), with holes punched around the
   * interactive objects. Double-clicking the band toggles shade (classic).
   */
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { open } from "@tauri-apps/plugin-dialog";

  import SkinSurface from "../components/skin/SkinSurface.svelte";
  import { queue, isAudioPath } from "../lib/player/queue.svelte";
  import type { EngineEvent } from "../lib/player/queue.svelte";
  import { loadDefaultSkin } from "../lib/skin/load";
  import { mockSkinPayload } from "../lib/skin/mock";
  import { resolveLayout } from "../lib/skin/resolver";
  import { formatWarning } from "../lib/skin/wasabi";
  import { dragRects } from "../lib/skin/titlebar";
  import type { SkinPayload } from "../lib/skin/types";
  import {
    playerLayoutMode,
    publishSkinTheme,
    setMediaCommandExecutor,
    windowIsVisible,
    windowSetSize,
    windowSetVisible,
    windowToggleLayout,
  } from "../lib/window/bus.svelte";

  type AudioState = {
    status: "stopped" | "playing" | "paused";
    positionSecs: number;
    durationSecs: number | null;
    volume: number;
    underrunFrames: number;
    /** Engine track metadata (classic kbps/kHz/mono-stereo readouts). */
    bitrateKbps: number | null;
    sampleRateHz: number | null;
    channels: number | null;
  };

  let state = $state<AudioState>({
    status: "stopped",
    positionSecs: 0,
    durationSecs: null,
    volume: 1,
    underrunFrames: 0,
    bitrateKbps: null,
    sampleRateHz: null,
    channels: null,
  });
  let seeking = $state<number | null>(null);

  // Feed errors inside the queue module surface on the console: the visible
  // M1 log moved to the library window's DEV block, and cross-window log
  // relay is deliberately not part of the bus contract.
  queue.setLog((line) => console.log(`[navinamp] ${line}`));

  async function refresh() {
    try {
      state = await invoke<AudioState>("audio_state");
    } catch {
      // backend without audio_state — keep the last state
    }
    pushMediaMetadata();
  }

  // Poll engine state at 10 Hz.
  $effect(() => {
    const timer = setInterval(refresh, 100);
    return () => clearInterval(timer);
  });

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

  // Engine events → queue correlation (the ONLY place that runs it).
  $effect(() => {
    const unlisten = listen<EngineEvent>("audio-event", (ev) => {
      const e = ev.payload;
      const note = queue.handleEngineEvent(e);
      console.log(`[navinamp] ${note ?? rawEngineLine(e)}`);
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
      console.warn(`[navinamp] ${name} error: ${e}`);
    }
    void refresh();
  }

  function warnIpc(e: unknown): void {
    console.warn(`[navinamp] window ipc: ${e}`);
  }

  // Multi-select audio dialog for EJECT.
  async function pickAudioPaths(): Promise<string[]> {
    let res: string | string[] | null = null;
    try {
      res = await open({ multiple: true, directory: false });
    } catch {
      return [];
    }
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

  const fmt = (secs: number) => {
    const m = Math.floor(secs / 60);
    const s = Math.floor(secs % 60);
    return `${m}:${String(s).padStart(2, "0")}`;
  };

  const duration = $derived(state.durationSecs ?? 0);
  const position = $derived(
    Math.min(seeking ?? state.positionSecs, duration || Infinity),
  );

  // Songticker content: "Artist - Title" of the current queue item.
  const ticker = $derived.by(() => {
    const it = queue.currentItem;
    return it ? (it.artist ? `${it.artist} - ${it.title}` : it.title) : "";
  });

  // ── skin loading: first installed .wal, else the built-in mock ──────────

  let skin = $state<SkinPayload | null>(null);
  let skinError = $state<string | null>(null);
  let scale = $state(1); // Alt+S toggles 1×/2× (no room for UI buttons here)

  async function loadInitialSkin(): Promise<SkinPayload> {
    try {
      return await loadDefaultSkin();
    } catch (e) {
      console.warn(`[navinamp] skin load failed (${e}; using mock)`);
      return mockSkinPayload();
    }
  }

  $effect(() => {
    let cancelled = false;
    loadInitialSkin()
      .then((payload) => {
        if (cancelled) return;
        skin = payload;
        // Share the skin's playlist colors (pledit.txt) with the other
        // windows; they theme the playlist editor from it.
        publishSkinTheme(payload.skin.params);
        const n = payload.warnings.length;
        if (n > 0) console.warn(`[navinamp] skin parsed with ${n} warning(s)`);
      })
      .catch((e) => {
        if (!cancelled) skinError = String(e);
      });
    return () => {
      cancelled = true;
    };
  });

  // ── acceptance-run diagnostics: skin warnings visible in-app ─────────────

  let warningsOpen = $state(false);
  const skinWarnings = $derived(
    (skin?.warnings ?? []).map((w) => formatWarning(w)),
  );
  const resolveWarnings = $derived(playerLayout?.warnings ?? []);
  const allWarnings = $derived([...skinWarnings, ...resolveWarnings]);

  // ── layout: follows the window manager's mode (normal/shade) ────────────

  const layoutMode = $derived(playerLayoutMode());
  const playerLayout = $derived(skin ? resolveLayout(skin, "main", layoutMode) : null);
  const dragStrips = $derived.by(() =>
    skin && playerLayout ? dragRects(skin.skin, playerLayout) : [],
  );

  // Size the OS window to the layout rect × scale. Fires on skin load, on
  // shade toggles (windows-changed flips layoutMode → re-resolve → new rect)
  // and on the Alt+S scale toggle.
  $effect(() => {
    const l = playerLayout;
    if (!l) return;
    void l.w;
    void l.h;
    void scale;
    windowSetSize("main", l.w * scale, l.h * scale).catch(warnIpc);
  });

  $effect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.altKey && (e.key === "s" || e.key === "S")) {
        e.preventDefault();
        scale = scale === 1 ? 2 : 1;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  // ── actions: standard Winamp button names → queue / audio IPC ───────────

  function stopEngine() {
    queue.onStopped();
    void cmd("audio_stop");
  }

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
        windowSetVisible("playlist", !windowIsVisible("playlist")).catch(warnIpc);
        break;
      case "TOGGLE_EQ": {
        const before = windowIsVisible("eq");
        console.warn(`[navinamp] TOGGLE_EQ click — visible before: ${before}`);
        windowSetVisible("eq", !before)
          .then(() => {
            console.warn(
              `[navinamp] TOGGLE_EQ ipc ok — visible now: ${windowIsVisible("eq")}`,
            );
          })
          .catch((e) => {
            console.warn(`[navinamp] TOGGLE_EQ ipc FAILED: ${e}`);
          });
        break;
      }
      case "TOGGLE_SHADE":
        windowToggleLayout("main").catch(warnIpc);
        break;
    }
  }

  // Media keys / DEV transport route through the same executor, so PLAY
  // resumes-when-paused exactly like the skin's play button.
  const MEDIA_TO_ACTION: Record<string, string> = {
    play: "PLAY",
    pause: "PAUSE",
    stop: "STOP",
    next: "NEXT",
    prev: "PREV",
  };
  setMediaCommandExecutor((c) => skinAction(MEDIA_TO_ACTION[c]));

  // ── OS media metadata (M6b) ─────────────────────────────────────────────
  // Pushed on track change (forced) and on ~1 Hz position ticks; playing
  // state transitions fire media_playback. Args are sent in BOTH spellings
  // because the contract writes them snake_case while Tauri commands read
  // camelCase keys by default — the backend picks whichever it declares and
  // ignores the other. Rejections (backend without the command yet) are
  // swallowed after the first warn.

  let metaWarned = false;
  let lastMetaKey = "";
  let lastPlaying: boolean | null = null;

  function mediaWarn(e: unknown): void {
    if (!metaWarned) {
      metaWarned = true;
      console.warn(`[navinamp] media metadata push unavailable: ${e}`);
    }
  }

  function pushMediaMetadata(force = false): void {
    const playing = state.status === "playing";
    if (playing !== lastPlaying) {
      lastPlaying = playing;
      invoke("media_playback", { playing }).catch(mediaWarn);
    }
    const item = queue.currentItem;
    if (!item) return;
    const pos = Math.floor(state.positionSecs);
    const key = `${item.id}:${item.durationSecs ?? "?"}:${pos}`;
    if (!force && key === lastMetaKey) return;
    lastMetaKey = key;
    const durationSecs = item.durationSecs ?? 0;
    invoke("media_metadata", {
      title: item.title,
      artist: item.artist,
      album: "", // QueueItem carries no album; the backend can enrich by path
      duration_secs: durationSecs,
      position_secs: pos,
      durationSecs,
      positionSecs: pos,
    }).catch(mediaWarn);
  }

  // Track change → full push (the 10 Hz poll drives the ~1 Hz tick path).
  $effect(() => {
    void queue.currentItem?.id;
    pushMediaMetadata(true);
  });
</script>

<div class="player">
  {#if playerLayout}
    <SkinSurface
      layout={playerLayout}
      {scale}
      status={state.status}
      {position}
      {duration}
      volume={state.volume}
      {ticker}
      playlistVisible={windowIsVisible("playlist")}
      eqVisible={windowIsVisible("eq")}
      shadeActive={layoutMode === "shade"}
      bitrateKbps={state.bitrateKbps}
      sampleRateHz={state.sampleRateHz}
      channels={state.channels}
      onaction={skinAction}
      onseek={(secs) => cmd("audio_seek", { secs })}
      onvolume={(v) => cmd("audio_set_volume", { volume: v })}
    />
    <div class="drag-layer" aria-hidden="true">
      {#each dragStrips as r (r.x)}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <!-- OS drag region (Tauri handles mousedown via the attribute); the
             dblclick is a classic shade toggle, not a DOM interaction. -->
        <div
          class="drag"
          data-tauri-drag-region
          style="left:{r.x * scale}px; top:{r.y * scale}px; width:{r.w * scale}px; height:{r.h * scale}px;"
          title="Drag to move — double-click toggles shade"
          ondblclick={() => windowToggleLayout("main").catch(warnIpc)}
        ></div>
      {/each}
    </div>
  {:else if skinError}
    <p class="status warn">skin load failed: {skinError}</p>
  {:else}
    <p class="status">loading skin…</p>
  {/if}

  {#if allWarnings.length > 0}
    <div class="warnbar">
      {#if warningsOpen}
        <div class="warnlist">
          {#each allWarnings as w}
            <div class="warnline">{w}</div>
          {/each}
        </div>
      {/if}
      <button class="warnbadge" onclick={() => (warningsOpen = !warningsOpen)}>
        ⚠ {allWarnings.length}
      </button>
    </div>
  {/if}
</div>

<style>
  .player {
    position: relative;
    background: #000;
    user-select: none;
  }

  /* Transparent layer above the skin; only the drag strips catch pointers. */
  .drag-layer {
    position: absolute;
    inset: 0;
    pointer-events: none;
    z-index: 10;
  }

  .drag {
    position: absolute;
    pointer-events: auto;
    cursor: default;
  }

  /* Acceptance-run diagnostics: skin warnings must be visible in-app. */
  .warnbar {
    position: absolute;
    left: 4px;
    bottom: 4px;
    z-index: 20;
    display: flex;
    flex-direction: column;
    gap: 4px;
    max-width: calc(100% - 8px);
  }

  .warnbadge {
    align-self: flex-start;
    background: #40241c;
    color: #d8604c;
    border: 1px solid #d8604c;
    border-radius: 3px;
    padding: 1px 6px;
    font: 10px ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    cursor: pointer;
  }

  .warnlist {
    background: rgba(0, 0, 0, 0.92);
    border: 1px solid #5a2c22;
    max-height: 140px;
    overflow-y: auto;
    padding: 4px 6px;
  }

  .warnline {
    font: 10px ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    color: #d8a63c;
    white-space: nowrap;
  }

  .status {
    margin: 1rem;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 0.85rem;
    color: #808080;
    letter-spacing: 0.15em;
  }

  .warn {
    color: #d8604c;
  }
</style>
