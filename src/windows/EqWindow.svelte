<script lang="ts">
  /**
   * The EQUALIZER window (label "eq", M6c) — renders the SKIN'S OWN `eq`
   * container when it has one (classic .wsz skins via the synthesizer emit
   * a full EQ: ON/AUTO/PRESETS buttons + preamp + 10 vertical EQ_SET
   * sliders), and falls back to the HTML EqPanel when the skin has no eq
   * container or no skin loaded (e.g. plain `vite dev`).
   *
   * EQ state ownership: unlike the queue (player-window-owned, bus-relayed),
   * the EQ lives in the ENGINE, so this window talks to it directly — a
   * cheap 2 Hz `eq_state` poll into a local store feeds the sliders and the
   * ON toggle, while slider drags fire `eq_set_band`/`eq_set_preamp`
   * fire-and-forget (the poll reconciles; same policy as the volume slider
   * and the HTML EqPanel).
   *
   * EQ_SET index mapping (frozen, mirrors the .wsz synthesizer): skin slider
   * param `index` 0 = PREAMP (`eq_set_preamp`), 1..10 = bands 60 Hz…16 kHz
   * (`eq_set_band(index-1)`).
   *
   * AUTO: UI-side auto-load policy. When enabled, each track change applies
   * the curve saved for that track, else the one saved for its artist, else
   * a preset named after its genre (built-in or user-saved). Track/artist
   * curves are saved from the library and playlist row context menus; the
   * AUTO flag persists in localStorage.
   */
  import {
    EQ_BAND_COUNT,
    eqPresets,
    eqSetBand,
    eqSetEnabled,
    eqSetPreamp,
    eqState,
    onPresetsChanged,
    type EqLive,
    type EqPreset,
  } from "../lib/audio/eq";
  import { getArtistEq, getTrackEq, onEqAssocChanged } from "../lib/audio/auto";
  import EqPanel from "../components/EqPanel.svelte";
  import SkinSurface from "../components/skin/SkinSurface.svelte";
  import { loadDefaultSkin, onSkinChanged } from "../lib/skin/load";
  import { mockSkinPayload } from "../lib/skin/mock";
  import { resolveLayout } from "../lib/skin/resolver";
  import { dragRects } from "../lib/skin/titlebar";
  import type { SkinPayload } from "../lib/skin/types";
  import { windowIsVisible, windowSetSize, queueSnapshot } from "../lib/window/bus.svelte";
  import { libraryTrack, type Track } from "../lib/library/ipc";
  import { LABEL } from "../lib/window/identity";
  import TitleStrip from "./TitleStrip.svelte";

  // ── skin loading: same persisted pick as the player; reload on switch ───

  let skin = $state<SkinPayload | null>(null);
  let scale = $state(1); // Alt+S toggles 1×/2× (same as the player window)
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
    if (seq === skinSeq) skin = payload;
  }

  $effect(() => {
    void reloadSkin();
  });

  $effect(() => {
    const unlisten = onSkinChanged(() => void reloadSkin());
    return () => {
      void unlisten.then((f) => f());
    };
  });

  // The `eq` container must exist explicitly: resolveLayout alone would
  // silently fall back to the skin's FIRST container (the player window).
  const hasEqContainer = $derived(
    skin?.skin.containers.some((c) => c.id === "eq" && c.layouts.length > 0) ?? false,
  );
  const eqLayout = $derived(
    hasEqContainer && skin ? resolveLayout(skin, "eq", "normal") : null,
  );
  const dragStrips = $derived.by(() =>
    skin && eqLayout ? dragRects(skin.skin, eqLayout) : [],
  );

  // ── live EQ state: 2 Hz poll + optimistic mutations ──────────────────────

  function normalize(bands: number[] | undefined | null): number[] {
    const out = Array<number>(EQ_BAND_COUNT).fill(0);
    if (Array.isArray(bands)) {
      for (let i = 0; i < EQ_BAND_COUNT; i++) out[i] = bands[i] ?? 0;
    }
    return out;
  }

  let eq = $state<EqLive>({
    enabled: false,
    preampDb: 0,
    bandsDb: Array<number>(EQ_BAND_COUNT).fill(0),
    auto: storedAuto(),
  });

  // AUTO = auto-load a preset whose name matches the current track's genre
  // (built-in curves like Rock/Jazz or user presets). The flag persists.
  function storedAuto(): boolean {
    try {
      return localStorage.getItem("navinamp.eq.auto") === "1";
    } catch {
      return false;
    }
  }

  function setAuto(on: boolean): void {
    eq.auto = on;
    try {
      localStorage.setItem("navinamp.eq.auto", on ? "1" : "0");
    } catch {
      // storage-less webview: session-only
    }
  }

  $effect(() => {
    const sync = async () => {
      try {
        const s = await eqState();
        eq.enabled = s.enabled;
        eq.preampDb = s.preampDb ?? 0;
        eq.bandsDb = normalize(s.bandsDb);
      } catch {
        // backend without the EQ commands — keep the last state
      }
    };
    void sync();
    const timer = setInterval(sync, 500);
    return () => clearInterval(timer);
  });

  let eqWarned = false;
  function warnEq(e: unknown): void {
    if (!eqWarned) {
      eqWarned = true;
      console.warn(`[navinamp] eq ipc unavailable: ${e}`);
    }
  }

  /** EQ_SET apply: skin index 0 = preamp, 1..10 = bands (db pre-rounded). */
  function eqSetFromSkin(skinIndex: number, db: number): void {
    if (skinIndex <= 0) {
      eq.preampDb = db; // optimistic; the poll reconciles
      eqSetPreamp(db).catch(warnEq);
    } else if (skinIndex <= EQ_BAND_COUNT) {
      eq.bandsDb[skinIndex - 1] = db;
      eqSetBand(skinIndex - 1, db).catch(warnEq);
    }
  }

  function toggleEnabled(): void {
    eq.enabled = !eq.enabled; // optimistic; the poll reconciles
    eqSetEnabled(eq.enabled).catch(warnEq);
  }

  // ── presets: PRESETS cycles through eq_presets(), applying each band ────
  // (Presets carry no preamp — the backend table is bands-only, so the
  // preamp slider deliberately stays where the user left it.)

  let presets: EqPreset[] = [];
  let presetIndex = $state(-1); // -1 = none applied yet

  function reloadPresets(): void {
    eqPresets()
      .then((p) => {
        presets = p;
        presetIndex = -1; // indexes may shift after a save/delete elsewhere
      })
      .catch(() => (presets = []));
  }

  $effect(() => {
    reloadPresets();
  });

  // The library window's EQ panel can save/delete user presets; refresh the
  // cycle so the new entries show up without reopening the window.
  $effect(() => {
    const unlisten = onPresetsChanged(reloadPresets);
    return () => {
      void unlisten.then((f) => f());
    };
  });

  function applyBands(bandsDb: number[], index = -1): void {
    // Applying a curve implies wanting to HEAR it: auto-enable the EQ stage
    // (it ships disabled — a disabled EQ makes presets inaudible).
    if (!eq.enabled) {
      eq.enabled = true;
      eqSetEnabled(true).catch(warnEq);
    }
    presetIndex = index;
    const bands = normalize(bandsDb);
    eq.bandsDb = bands;
    bands.forEach((db, i) => eqSetBand(i, db).catch(warnEq));
  }

  function cyclePreset(): void {
    if (presets.length === 0) return;
    const next = (presetIndex + 1) % presets.length;
    applyBands(normalize(presets[next]?.bandsDb), next);
  }

  // ── AUTO: track curve → artist curve → genre-named preset ───────────────
  // The engine has no AUTO concept; this is a UI policy. Track/artist curves
  // are captured from the library/playlist context menus; the genre fallback
  // uses a preset named after the track's genre (built-in or user save).
  // Tracks with no match leave the curve alone.

  let lastAutoPath = "";
  let autoSeq = 0;

  async function applyAutoEq(path: string, metaArtist: string): Promise<void> {
    const seq = ++autoSeq;

    // 1. Track curve — applies even when the path has no library row.
    const trackCurve = getTrackEq(path);
    if (trackCurve) {
      applyBands(trackCurve.bandsDb);
      return;
    }

    let track: Track | null = null;
    try {
      track = await libraryTrack(path);
    } catch {
      // unknown/unsynced track: keep the queue metadata as the fallback
    }
    if (seq !== autoSeq) return;

    // 2. Artist curve.
    const artist = (track?.artist || metaArtist || "").trim();
    const artistCurve = getArtistEq(artist);
    if (artistCurve) {
      applyBands(artistCurve);
      return;
    }

    // 3. Genre-named preset.
    const genre = track?.genre ?? "";
    if (genre === "") return;
    if (presets.length === 0) {
      try {
        presets = await eqPresets();
      } catch {
        return;
      }
      if (seq !== autoSeq) return;
    }
    const index = presets.findIndex(
      (p) => p.name.toLowerCase() === genre.toLowerCase(),
    );
    if (index < 0) return;
    applyBands(normalize(presets[index]?.bandsDb), index);
  }

  $effect(() => {
    const snap = queueSnapshot.value;
    const item = snap?.currentIndex != null ? snap.items[snap.currentIndex] : null;
    if (!eq.auto) {
      lastAutoPath = ""; // apply again when AUTO is re-enabled
      return;
    }
    if (!item) {
      lastAutoPath = ""; // queue cleared: a re-play should re-apply
      return;
    }
    if (item.path === lastAutoPath) return;
    lastAutoPath = item.path;
    void applyAutoEq(item.path, item.artist);
  });

  // Saving/clearing a curve in another window re-evaluates the current track
  // only when the change affects it (a curve saved for another row must not
  // clobber manual tweaks on this one).
  $effect(() => {
    const unlisten = onEqAssocChanged((change) => {
      if (!eq.auto) return;
      const snap = queueSnapshot.value;
      const item = snap?.currentIndex != null ? snap.items[snap.currentIndex] : null;
      if (!item) return;
      const sameArtist =
        change.artist.trim() !== "" &&
        change.artist.trim().toLowerCase() === item.artist.trim().toLowerCase();
      const affects =
        change.scope === "track"
          ? change.path === item.path
          : change.path === item.path || sameArtist;
      if (!affects) return;
      lastAutoPath = item.path;
      void applyAutoEq(item.path, item.artist);
    });
    return () => {
      void unlisten.then((f) => f());
    };
  });

  // ── actions from the skin's EQ objects ───────────────────────────────────
  // EQ_TOGGLE/EQ_AUTO/EQ_PRESETS are canonical (the .wsz synthesizer emits
  // them); ON/AUTO/PRESETS are accepted aliases.

  function skinAction(action: string) {
    switch (action.trim().toUpperCase()) {
      case "EQ_TOGGLE":
      case "ON":
        toggleEnabled();
        break;
      case "EQ_AUTO":
      case "AUTO":
        setAuto(!eq.auto);
        break;
      case "EQ_PRESETS":
      case "PRESETS":
        cyclePreset();
        break;
    }
  }

  // ── window sizing: ResizeObserver on the root (mount / layout / scale) ──

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
</script>

<div class="win" bind:this={root}>
  {#if eqLayout}
    <SkinSurface
      layout={eqLayout}
      {scale}
      playlistVisible={windowIsVisible("playlist")}
      eqVisible={windowIsVisible("eq")}
      {eq}
      onaction={skinAction}
      oneqset={eqSetFromSkin}
    />
    <div class="drag-layer" aria-hidden="true">
      {#each dragStrips as r (r.x)}
        <!-- OS drag region (Tauri handles mousedown via the attribute). No
             shade toggle here — the EQ window has no shade layout. -->
        <div
          class="drag"
          data-tauri-drag-region
          style="left:{r.x * scale}px; top:{r.y * scale}px; width:{r.w * scale}px; height:{r.h * scale}px;"
        ></div>
      {/each}
    </div>
  {:else}
    <!-- No skin / no eq container: the HTML panel (same IPC wiring). -->
    <div class="fallback">
      <TitleStrip title="EQUALIZER" />
      <EqPanel />
    </div>
  {/if}
</div>

<style>
  .win {
    position: relative;
    width: max-content;
    background: #000;
    user-select: none;
  }

  /* HTML fallback column: fixed width so the ResizeObserver settles once. */
  .fallback {
    width: 344px;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 6px;
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
</style>
