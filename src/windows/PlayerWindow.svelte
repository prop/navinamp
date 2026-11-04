<script lang="ts">
  /**
   * The PLAYER window (label "main") — the skin surface, and the single
   * owner of playback state (M6b).
   *
   * Everything stateful lives HERE and only here: the queue module, the
   * 10 Hz `audio_state` poll, the engine-event listener (queue feed-order
   * correlation), the spectrum poller (via SkinVis), the OS media-metadata
   * push and the desktop "now playing" notification gate. The other
   * windows are bus-driven views.
   *
   * Layout: the resolved layout follows the window manager's layout mode
   * (normal / shade; a skin without a shade layout degrades to normal via
   * the resolver), and the OS window is sized to the layout rect × scale —
   * which is also how a shade toggle resizes the window.
   *
   * Dragging: the skin draws its own titlebar, so a transparent layer
   * overlays the titlebar band discovered from the skin (lib/skin/titlebar),
   * with holes punched around the interactive objects. The layer starts the
   * OS drag only after the pointer moves (lib/window/drag), leaving the
   * double-click to toggle shade (classic).
   */
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { open } from "@tauri-apps/plugin-dialog";

  import SkinSurface from "../components/skin/SkinSurface.svelte";
  import OnboardingLauncher from "../components/OnboardingLauncher.svelte";
  import { queue, isAudioPath } from "../lib/player/queue.svelte";
  import type { EngineEvent } from "../lib/player/queue.svelte";
  import {
    artForTrack,
    localArtUrl,
    navidromeArtUrl,
  } from "../lib/library/ipc";
  import { loadDefaultSkin, onSkinChanged } from "../lib/skin/load";
  import { mockSkinPayload } from "../lib/skin/mock";
  import { resolveLayout } from "../lib/skin/resolver";
  import { formatWarning } from "../lib/skin/wasabi";
  import { dragRects } from "../lib/skin/titlebar";
  import { dragWindow } from "../lib/window/drag";
  import type { SkinPayload, Warning } from "../lib/skin/types";
  import {
    appFocused,
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
    /** Desktop "now playing" notifications enabled (menu preference). */
    notifyTrack: boolean;
  };

  let audio = $state<AudioState>({
    status: "stopped",
    positionSecs: 0,
    durationSecs: null,
    volume: 1,
    underrunFrames: 0,
    bitrateKbps: null,
    sampleRateHz: null,
    channels: null,
    notifyTrack: true,
  });
  let seeking = $state<number | null>(null);

  // Long-press transport seek: 1s per 100ms tick while PREV/NEXT is held.
  // The end margin keeps fast-forward from tripping TrackFinished.
  const HOLD_SEEK_STEP_SECS = 1;
  const HOLD_SEEK_TICK_MS = 100;
  const HOLD_SEEK_END_MARGIN_SECS = 0.1;
  let holdTimer: ReturnType<typeof setInterval> | null = null;
  let holdTarget = 0;
  let holdItemId: string | null = null;

  // Feed errors inside the queue module surface on the console: the visible
  // M1 log moved to the library window's DEV block, and cross-window log
  // relay is deliberately not part of the bus contract.
  queue.setLog((line) => console.log(`[navinamp] ${line}`));

  async function refresh() {
    try {
      audio = await invoke<AudioState>("audio_state");
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

  // ── desktop notifications (issue #34) ────────────────────────────────────
  // TrackStarted — not queue edits or the persisted-queue restore — is the
  // trigger. Automatic toasts require the menu preference and "no NaVinaMP
  // window focused"; the native global shortcut is an explicit request and
  // bypasses those gates.
  //
  // Sending goes through the Rust side (`notify_track_started` → notify-rust
  // → UNUserNotificationCenter): the webview's Web Notification API ignores
  // images and cannot be granted permission in WKWebView, while the Rust
  // path carries album art as a native attachment and lets the OS handle
  // authorization (prompt on the first notification, remembered after).
  let notifyWarned = false;

  /** Album art as base64 for the notification attachment; null = no art. */
  async function fetchNotifyArt(): Promise<string | null> {
    const item = queue.currentItem;
    if (!item) return null;
    // Navidrome tracks use the server's scaled getCoverArt; local tracks the
    // embedded-art cache. Any failure just means no art on the toast.
    const url =
      item.remoteId != null
        ? await navidromeArtUrl(item.serverId, item.remoteId, 300)
        : localArtUrl(await artForTrack(item.path) ?? "");
    if (!url) return null;
    const resp = await fetch(url);
    if (!resp.ok) return null;
    const bytes = new Uint8Array(await resp.arrayBuffer());
    let binary = "";
    const CHUNK = 0x8000;
    for (let i = 0; i < bytes.length; i += CHUNK) {
      binary += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
    }
    return btoa(binary);
  }

  async function showNowPlaying(manual = false): Promise<void> {
    // Automatic notifications obey the menu preference and suppress while
    // the app is focused. The global shortcut is an explicit request, so it
    // raises the toast even then.
    if (!manual && (!audio.notifyTrack || appFocused())) return;
    const item = queue.currentItem;
    if (!item) {
      if (manual) console.warn("[navinamp] notify shortcut: nothing is playing");
      return;
    }
    try {
      const artBase64 = await fetchNotifyArt().catch(() => null);
      await invoke("notify_track_started", {
        title: item.artist ? `${item.artist} — ${item.title}` : item.title,
        artBase64,
      });
    } catch (e) {
      if (!notifyWarned) {
        notifyWarned = true;
        console.warn(`[navinamp] notifications unavailable: ${e}`);
      }
    }
  }

  // Engine events → queue correlation (the ONLY place that runs it).
  $effect(() => {
    const unlisten = listen<EngineEvent>("audio-event", (ev) => {
      const e = ev.payload;
      const note = queue.handleEngineEvent(e);
      console.log(`[navinamp] ${note ?? rawEngineLine(e)}`);
      if (e.type === "TrackStarted") void showNowPlaying();
      void refresh();
    });
    return () => {
      void unlisten.then((f) => f());
    };
  });

  // Native global shortcut (registered in Rust) asks for the current track
  // even when another app is focused.
  $effect(() => {
    const unlisten = listen("notify-now-request", () => void showNowPlaying(true));
    return () => {
      void unlisten.then((f) => f());
    };
  });

  async function cmd(name: string, ...args: unknown[]) {
    try {
      await invoke(
        name,
        args.length > 0 ? (args[0] as Record<string, unknown>) : undefined,
      );
    } catch (e) {
      console.warn(`[navinamp] ${name} error: ${e}`);
    }
    void refresh();
  }

  function warnIpc(e: unknown): void {
    console.warn(`[navinamp] window ipc: ${e}`);
  }

  // ── seeking (media keys / long-press transport) ──────────────────────────

  /** Clamp a target into the current track, keeping a hair off the end. */
  function clampHoldTarget(secs: number): number {
    const total = audio.durationSecs ?? 0;
    if (total <= 0) return 0;
    return Math.min(Math.max(secs, 0), Math.max(0, total - HOLD_SEEK_END_MARGIN_SECS));
  }

  /** One-shot seek for OS media seek/scrub events. */
  function seekTo(secs: number): void {
    if (!Number.isFinite(secs) || duration <= 0 || audio.status === "stopped") return;
    const target = Math.min(Math.max(secs, 0), duration);
    void cmd("audio_seek", { secs: target });
  }

  function seekBy(deltaSecs: number): void {
    if (!Number.isFinite(deltaSecs)) return;
    seekTo(position + deltaSecs);
  }

  /**
   * Start a press-and-hold seek. Returns false when the engine can't seek
   * (stopped or unknown duration), letting the button fall back to a click.
   */
  function startSeekHold(action: string): boolean {
    const normalized = action.trim().toUpperCase();
    const delta = normalized === "NEXT" ? 1 : normalized === "PREV" ? -1 : 0;
    if (delta === 0) return false;
    if (holdTimer != null) return true;
    if (audio.status === "stopped" || (audio.durationSecs ?? 0) <= 0) return false;

    holdTarget = clampHoldTarget(audio.positionSecs);
    holdItemId = queue.currentItem?.id ?? null;
    const tick = () => {
      // A gapless advance / explicit next-prev during the hold must not
      // continue seeking inside the newly started track.
      if (
        audio.status === "stopped" ||
        (audio.durationSecs ?? 0) <= 0 ||
        (queue.currentItem?.id ?? null) !== holdItemId
      ) {
        stopSeekHold();
        return;
      }
      const next = clampHoldTarget(holdTarget + delta * HOLD_SEEK_STEP_SECS);
      if (next === holdTarget) {
        // At the track boundary: stop re-seeking and keep the position put.
        if (holdTimer != null) {
          clearInterval(holdTimer);
          holdTimer = null;
        }
        return;
      }
      holdTarget = next;
      seeking = holdTarget;
      void cmd("audio_seek", { secs: holdTarget });
    };
    // Arm the interval before the first tick so a boundary stop can clear it.
    // The immediate step gives feedback as soon as the hold threshold is
    // crossed; otherwise a quick hold-release would swallow the click and
    // move nothing.
    holdTimer = setInterval(tick, HOLD_SEEK_TICK_MS);
    tick();
    return true;
  }

  function stopSeekHold(): void {
    if (holdTimer != null) {
      clearInterval(holdTimer);
      holdTimer = null;
    }
    holdItemId = null;
    if (seeking != null) {
      seeking = null;
      void refresh();
    }
  }

  // A blurred window loses the pointer-up; never leave a seek loop running.
  $effect(() => {
    window.addEventListener("blur", stopSeekHold);
    return () => window.removeEventListener("blur", stopSeekHold);
  });

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

  const duration = $derived(audio.durationSecs ?? 0);
  const position = $derived(
    Math.min(seeking ?? audio.positionSecs, duration || Infinity),
  );

  // Songticker content: "Artist - Title" of the current queue item.
  const ticker = $derived.by(() => {
    const it = queue.currentItem;
    return it ? (it.artist ? `${it.artist} - ${it.title}` : it.title) : "";
  });

  // ── skin loading: persisted choice, else first installed, else the mock ──

  let skin = $state<SkinPayload | null>(null);
  let skinError = $state<string | null>(null);
  let scale = $state(1); // Alt+S toggles 1×/2× (no room for UI buttons here)
  let skinSeq = 0;

  async function reloadSkin(): Promise<void> {
    const seq = ++skinSeq;
    let payload: SkinPayload;
    try {
      payload = await loadDefaultSkin();
    } catch (e) {
      console.warn(`[navinamp] skin load failed (${e}; using mock)`);
      payload = mockSkinPayload();
    }
    if (seq !== skinSeq) return; // a newer switch superseded this load
    skin = payload;
    skinError = null;
    // Share the skin's playlist colors (pledit.txt) with the other windows;
    // they theme the playlist editor from it.
    publishSkinTheme(payload.skin.params);
    const n = payload.warnings.length;
    if (n > 0) console.warn(`[navinamp] skin parsed with ${n} warning(s)`);
  }

  // Initial load + live switching from the library's skin selector.
  $effect(() => {
    void reloadSkin();
  });

  $effect(() => {
    const unlisten = onSkinChanged(() => void reloadSkin());
    return () => {
      void unlisten.then((f) => f());
    };
  });

  // ── acceptance-run diagnostics: skin warnings visible in-app ─────────────

  let warningsOpen = $state(false);
  const skinWarnings = $derived(
    (skin?.warnings ?? []).map((w: Warning) => formatWarning(w)),
  );

  // ── layout: follows the window manager's mode (normal/shade) ────────────

  const layoutMode = $derived(playerLayoutMode());
  const playerLayout = $derived(skin ? resolveLayout(skin, "main", layoutMode) : null);
  const resolveWarnings = $derived(playerLayout?.warnings ?? []);
  const allWarnings = $derived([...skinWarnings, ...resolveWarnings]);
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
      if (!e.altKey) return;
      if (e.key === "s" || e.key === "S") {
        e.preventDefault();
        scale = scale === 1 ? 2 : 1;
      } else if (e.key === "l" || e.key === "L") {
        // Universal library opener: classic .wsz skins have no LIBRARY
        // button (Winamp 2 predates the media library), so the shortcut is
        // the fallback for every skin.
        e.preventDefault();
        windowSetVisible("library", !windowIsVisible("library")).catch(warnIpc);
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
        if (audio.status === "paused") void cmd("audio_resume");
        else if (audio.status === "stopped" && queue.items.length > 0) {
          queue.play(queue.currentIndex ?? 0);
        }
        break;
      case "PAUSE":
        if (audio.status === "playing") void cmd("audio_pause");
        else if (audio.status === "paused") void cmd("audio_resume");
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
      case "TOGGLE_LIBRARY":
        windowSetVisible("library", !windowIsVisible("library")).catch(warnIpc);
        break;
      case "TOGGLE_EQ":
        windowSetVisible("eq", !windowIsVisible("eq")).catch(warnIpc);
        break;
      case "TOGGLE_SHUFFLE":
        queue.toggleShuffle();
        break;
      case "TOGGLE_REPEAT":
        queue.cycleRepeat();
        break;
      case "TOGGLE_SHADE":
        windowToggleLayout("main").catch(warnIpc);
        break;
    }
  }

  // Media keys / DEV transport route through the same executor, so PLAY
  // resumes-when-paused exactly like the skin's play button. OS seek events
  // (remote seek buttons / scrubbers) are absolute or relative seeks.
  const MEDIA_TO_ACTION: Record<string, string> = {
    play: "PLAY",
    pause: "PAUSE",
    stop: "STOP",
    next: "NEXT",
    prev: "PREV",
  };
  setMediaCommandExecutor((c, secs) => {
    if (c === "seekBy") {
      seekBy(secs ?? 0);
      return;
    }
    if (c === "seekTo") {
      seekTo(secs ?? 0);
      return;
    }
    skinAction(MEDIA_TO_ACTION[c]);
  });

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
    const playing = audio.status === "playing";
    if (playing !== lastPlaying) {
      lastPlaying = playing;
      invoke("media_playback", { playing }).catch(mediaWarn);
    }
    const item = queue.currentItem;
    if (!item) return;
    const pos = Math.floor(audio.positionSecs);
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
      status={audio.status}
      {position}
      {duration}
      volume={audio.volume}
      {ticker}
      playlistVisible={windowIsVisible("playlist")}
      libraryVisible={windowIsVisible("library")}
      eqVisible={windowIsVisible("eq")}
      shuffle={queue.shuffle}
      repeatOn={queue.repeat !== "off"}
      shadeActive={layoutMode === "shade"}
      bitrateKbps={audio.bitrateKbps}
      sampleRateHz={audio.sampleRateHz}
      channels={audio.channels}
      onaction={skinAction}
      onholdstart={startSeekHold}
      onholdend={() => stopSeekHold()}
      onseek={(secs) => cmd("audio_seek", { secs })}
      onvolume={(v) => cmd("audio_set_volume", { volume: v })}
    />
    <div class="drag-layer" aria-hidden="true">
      {#each dragStrips as r (r.x)}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <!-- Threshold drag keeps the double-click for the classic shade
             toggle (a Tauri drag region would maximize the window). -->
        <div
          class="drag"
          use:dragWindow
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

  <OnboardingLauncher />
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
