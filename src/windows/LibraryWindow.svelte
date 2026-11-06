<script lang="ts">
  /**
   * The LIBRARY window (label "library") — the M1-M5 app surface behind
   * one tab strip (#16): LIBRARY (Media Library with its Navidrome
   * settings + smart-view modals), EQUALIZER (the shared EqPanel with
   * the AUTO CURVES manager), SKIN (the skin switcher) and DEV (the M1
   * debug surface).
   *
   * The LIBRARY pane stays mounted (hidden while inactive) so its
   * selection, search, scroll and loaded pages survive tab round-trips;
   * EQUALIZER/DEV mount only while active, so their pollers never run
   * in the background. The active tab persists in localStorage.
   *
   * Queue hooks are intents: ENQUEUE/PLAY append via the player; the queue
   * status line in the title strip renders from snapshots.
   *
   * The DEV pane keeps the M1 engine probes: transport buttons go through
   * `media-command` (relayed to the player, whose executor applies the same
   * semantics as the skin buttons — a raw `audio_stop` from here would
   * desync the player's feed-order correlation), while the file dialogs and
   * seek/volume sliders stay direct IPC like before.
   */
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { open } from "@tauri-apps/plugin-dialog";

  import EqPanel from "../components/EqPanel.svelte";
  import MediaLibrary from "../components/MediaLibrary.svelte";
  import TabStrip from "../components/TabStrip.svelte";
  import TourHost from "../components/TourHost.svelte";
  import {
    POPULAR_PROGRESS_EVENT,
    chooseSkin,
    installPopularSkins,
    listSkins,
    loadSkin,
    popularSkins,
    selectedSkinId,
    type PopularPack,
    type PopularProgress,
    type SkinInfo,
  } from "../lib/skin/load";
  import {
    emitMediaCommand,
    queueIntent,
    queuePlayNow,
    queueReplaceAll,
    queueSnapshot,
    sizeWindowTo,
  } from "../lib/window/bus.svelte";
  import { LABEL } from "../lib/window/identity";
  import TitleStrip from "./TitleStrip.svelte";

  const snap = $derived(queueSnapshot.value);

  const queueNote = $derived(
    `QUEUE ${snap?.currentIndex == null ? "--" : snap.currentIndex + 1}/${snap?.items.length ?? 0}`,
  );

  // ── tabs (#16): LIBRARY | EQUALIZER | SKIN | DEV; active tab persists ───

  const TAB_KEY = "navinamp.library.tab";

  const TABS = [
    { id: "library", label: "LIBRARY" },
    { id: "eq", label: "EQUALIZER" },
    { id: "skin", label: "SKIN" },
    { id: "dev", label: "DEV" },
  ] as const;

  type TabId = (typeof TABS)[number]["id"];

  function storedTab(): TabId {
    const raw = localStorage.getItem(TAB_KEY);
    return TABS.some((t) => t.id === raw) ? (raw as TabId) : "library";
  }

  let tab = $state<TabId>(storedTab());

  $effect(() => {
    localStorage.setItem(TAB_KEY, tab);
  });

  // ── skin switcher (validates, persists, broadcasts; player + EQ reload) ──

  let skins = $state.raw<SkinInfo[]>([]);
  let skinChoice = $state("");
  let skinBusy = $state(false);
  let skinNote = $state<string | null>(null);
  let skinError = $state<string | null>(null);

  /** Prefer the persisted choice, then the loader's default pick. */
  function currentSkinId(list: SkinInfo[]): string {
    const wanted = selectedSkinId();
    if (wanted != null && list.some((s) => s.id === wanted)) return wanted;
    return list.find((s) => s.id !== "fallback")?.id ?? list[0]?.id ?? "";
  }

  async function refreshSkins(): Promise<void> {
    try {
      const list = await listSkins();
      skins = list;
      skinChoice = currentSkinId(list);
      skinError = null;
    } catch (e) {
      skinError = `skin list: ${e instanceof Error ? e.message : String(e)}`;
    }
  }

  // ── popular skins pack (downloaded on request from archive.org) ──────────

  let popular = $state.raw<PopularPack | null>(null);
  let popularBusy = $state(false);
  let popularNote = $state<string | null>(null);
  let popularError = $state<string | null>(null);
  let popularProgress = $state<PopularProgress | null>(null);

  const popularInstalled = $derived(popular?.skins.filter((s) => s.installed).length ?? 0);
  const popularTotal = $derived(popular?.skins.length ?? 0);
  const popularMissing = $derived(popularTotal - popularInstalled);

  function skinLabel(id: string): string {
    return popular?.skins.find((s) => s.id === id)?.name ?? id;
  }

  async function refreshPopular(): Promise<void> {
    try {
      popular = await popularSkins();
      popularError = null;
    } catch (e) {
      popularError = `popular skins: ${e instanceof Error ? e.message : String(e)}`;
    }
  }

  async function installPopular(): Promise<void> {
    if (popularBusy || popular == null) return;
    popularBusy = true;
    popularNote = null;
    popularError = null;
    try {
      const report = await installPopularSkins();
      popularNote =
        `${report.installed} installed · ${report.skipped} already present` +
        (report.failed.length > 0 ? ` · ${report.failed.length} failed` : "");
      const failures = report.failed.map((f) => `${f.id}: ${f.error}`).join("; ");
      await Promise.all([refreshPopular(), refreshSkins()]);
      // Set after the refresh (which clears the previous error).
      if (failures !== "") popularError = failures;
      logLine(`popular skins: ${popularNote}`);
    } catch (e) {
      popularError = `popular skins: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      popularBusy = false;
    }
  }

  // Load the pack when the SKIN tab becomes visible; stream install progress.
  $effect(() => {
    if (tab !== "skin") return;
    void refreshPopular();
  });

  $effect(() => {
    const unlisten = listen<PopularProgress>(POPULAR_PROGRESS_EVENT, (event) => {
      popularProgress = event.payload;
      if (event.payload.error != null) {
        popularError = `${event.payload.name || event.payload.id}: ${event.payload.error}`;
      }
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  });

  async function applySkin(): Promise<void> {
    if (skinChoice === "" || skinBusy) return;
    skinBusy = true;
    skinNote = null;
    skinError = null;
    try {
      // Validate + warm the backend cache before telling the windows.
      await loadSkin(skinChoice);
      await chooseSkin(skinChoice);
      skinNote = `applied ${skinChoice}`;
      logLine(`skin: ${skinChoice}`);
    } catch (e) {
      skinError = `skin: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      skinBusy = false;
    }
  }

  $effect(() => {
    void refreshSkins();
  });

  let log = $state<string[]>([]);

  function logLine(line: string) {
    log = [`${new Date().toLocaleTimeString()}  ${line}`, ...log].slice(0, 8);
  }

  // ── DEV pane (M1 debug surface) ──────────────────────────────────────────

  type AudioState = {
    status: "stopped" | "playing" | "paused";
    positionSecs: number;
    durationSecs: number | null;
    volume: number;
    underrunFrames: number;
    bitrateKbps: number | null;
    sampleRateHz: number | null;
    channels: number | null;
  };
  let devState = $state<AudioState>({
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

  // The 10 Hz audio poll runs only while the DEV tab is visible (#16).
  $effect(() => {
    if (tab !== "dev") return;
    const timer = setInterval(async () => {
      try {
        devState = await invoke<AudioState>("audio_state");
      } catch {
        // backend without audio_state — keep the last state
      }
    }, 100);
    return () => clearInterval(timer);
  });

  async function devCmd(name: string, args?: Record<string, unknown>): Promise<void> {
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
      sizeWindowTo(LABEL, w, h);
    };
    apply();
    const ro = new ResizeObserver(apply);
    ro.observe(el);
    return () => ro.disconnect();
  });
</script>

<div class="win" bind:this={root}>
  <TitleStrip title="MEDIA LIBRARY" note={queueNote} />

  <TabStrip tabs={TABS} active={tab} onselect={(id) => (tab = id as TabId)} />

  <!-- LIBRARY stays mounted (hidden while inactive): selection, search,
       scroll position and loaded pages survive a tab round-trip (#16). -->
  <div id="pane-library" class="pane" role="tabpanel" aria-labelledby="tab-library" class:off={tab !== "library"}>
    <MediaLibrary
      onenqueue={(metas) => queueIntent({ type: "addFromLibrary", metas })}
      onplaynow={(metas) => queuePlayNow(metas)}
      onreplace={(metas) => queueReplaceAll(metas)}
      onlog={logLine} />
  </div>

  {#if tab === "eq"}
    <div id="pane-eq" class="pane" role="tabpanel" aria-labelledby="tab-eq">
      <EqPanel manageCurves />
    </div>
  {/if}

  {#if tab === "skin"}
    <section id="pane-skin" class="pane skinpane" role="tabpanel" aria-labelledby="tab-skin">
      <div class="skinbar">
        <span class="skinlabel">skin</span>
        <select bind:value={skinChoice} aria-label="skin" onchange={() => (skinNote = null)}>
          {#each skins as s (s.id)}
            <option value={s.id}>{skinLabel(s.id)}{s.id === "fallback" ? " (built-in)" : ""}</option>
          {/each}
        </select>
        <button onclick={() => void applySkin()} disabled={skinBusy || skins.length === 0}>
          {skinBusy ? "APPLYING…" : "APPLY"}
        </button>
        <button class="reload" title="rescan the skins folder" onclick={() => void refreshSkins()}
          >↻</button>
        {#if skinNote}<span class="ok">{skinNote}</span>{/if}
        {#if skinError}<span class="err">{skinError}</span>{/if}
      </div>

      <div class="popular">
        <div class="poprow">
          <button
            onclick={() => void installPopular()}
            disabled={popularBusy || popular == null || popularMissing === 0}>
            {popularBusy
              ? "DOWNLOADING…"
              : popular == null
                ? "CHECKING PACK…"
                : popularMissing > 0
                  ? `INSTALL POPULAR SKINS (${popularMissing})`
                  : "POPULAR SKINS INSTALLED"}
          </button>
          <button
            class="reload"
            title="recheck the installed pack files"
            onclick={() => void refreshPopular()}
            disabled={popularBusy}>↻</button>
          {#if popularBusy && popularProgress}
            <span class="dim">
              {popularProgress.done}/{popularProgress.total}
              {#if popularProgress.name} · {popularProgress.name}{/if}
            </span>
          {:else if popularNote}
            <span class="ok">{popularNote}</span>
          {/if}
        </div>
        <div class="popnote">
          {popularInstalled}/{popularTotal} installed · curated {popular?.curatedAt ?? "…"} from the
          <strong>Winamp Skin Museum</strong> (archive.org).
          {popular?.licenseNote ?? "Skins download only when you ask; authors retain their rights."}
        </div>
        {#if popularError}<span class="err">{popularError}</span>{/if}
      </div>
    </section>
  {/if}

  {#if tab === "dev"}
    <section id="pane-dev" class="pane dev" role="tabpanel" aria-labelledby="tab-dev">
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
          {#if devState.bitrateKbps != null || devState.sampleRateHz != null}
            <span class="meta">
              {devState.bitrateKbps ?? "—"} kbps ·
              {devState.sampleRateHz != null
                ? (devState.sampleRateHz / 1000).toFixed(1)
                : "—"} kHz ·
              {devState.channels != null
                ? devState.channels === 1
                  ? "mono"
                  : "stereo"
                : "—"}
            </span>
          {/if}
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
    </section>
  {/if}

  <TourHost onensurelibrary={() => (tab = "library")} />
</div>

<style>
  .win {
    width: max-content; /* widest pane (640px) + padding */
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 6px;
    background: #000;
    color: #c8c8c8;
    font-family: "Segoe UI", "Helvetica Neue", Helvetica, Arial, sans-serif;
    user-select: none;
  }

  /* One pane per tab; the OS window is sized to the active pane's natural
     height by the ResizeObserver above (#16). The LIBRARY pane is merely
     hidden (not unmounted) so its state survives tab round-trips. */
  .pane {
    width: 640px;
    box-sizing: border-box;
  }

  .pane.off {
    display: none;
  }

  .dev {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.5rem;
    padding-top: 0.75rem;
    border-top: 1px solid #222;
  }

  .skinbar {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
    padding-top: 0.75rem;
    border-top: 1px solid #222;
    font-size: 0.8rem;
    color: #808080;
  }

  .skinbar select {
    background: #1a1a1a;
    color: #c8c8c8;
    border: 1px solid #3a3a3a;
    border-radius: 3px;
    padding: 0.25rem 0.4rem;
    font-size: 0.8rem;
    max-width: 240px;
  }

  .skinbar button {
    background: #1a1a1a;
    color: #c8c8c8;
    border: 1px solid #3a3a3a;
    border-radius: 3px;
    padding: 0.25rem 0.7rem;
    font-size: 0.8rem;
    cursor: pointer;
  }

  .skinbar button:hover:not(:disabled) {
    border-color: #3ccb4e;
    color: #3ccb4e;
  }

  .skinbar button:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .skinbar .ok {
    color: #3ccb4e;
  }

  .skinbar .err {
    color: #d8604c;
  }

  .skinpane {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .popular {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.45rem;
    padding-top: 0.75rem;
    border-top: 1px solid #222;
    font-size: 0.8rem;
    color: #808080;
  }

  .poprow {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }

  .popular button {
    background: #1a1a1a;
    color: #c8c8c8;
    border: 1px solid #3a3a3a;
    border-radius: 3px;
    padding: 0.3rem 0.8rem;
    font-size: 0.8rem;
    cursor: pointer;
  }

  .popular button:hover:not(:disabled) {
    border-color: #3ccb4e;
    color: #3ccb4e;
  }

  .popular button:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .popular .reload {
    padding: 0.3rem 0.55rem;
  }

  .popular .dim {
    color: #707070;
  }

  .popular .ok {
    color: #3ccb4e;
  }

  .popular .err {
    max-width: 90%;
    text-align: center;
    color: #d8604c;
  }

  .popnote {
    max-width: 92%;
    font-size: 0.75rem;
    line-height: 1.45;
    text-align: center;
    color: #707070;
  }

  .popnote strong {
    color: #9a9a9a;
    font-weight: 600;
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

  .meta {
    margin-left: 0.75rem;
    letter-spacing: 0.02em;
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
