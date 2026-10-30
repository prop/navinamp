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
   * AUTO: visual toggle only — there is no backend AUTO concept yet, so the
   * flag is window-local and NOT persisted (documented follow-up).
   */
  import {
    EQ_BAND_COUNT,
    eqPresets,
    eqSetBand,
    eqSetEnabled,
    eqSetPreamp,
    eqState,
    type EqLive,
    type EqPreset,
  } from "../lib/audio/eq";
  import EqPanel from "../components/EqPanel.svelte";
  import SkinSurface from "../components/skin/SkinSurface.svelte";
  import { loadDefaultSkin } from "../lib/skin/load";
  import { mockSkinPayload } from "../lib/skin/mock";
  import { resolveLayout } from "../lib/skin/resolver";
  import { dragRects } from "../lib/skin/titlebar";
  import type { SkinPayload } from "../lib/skin/types";
  import { windowIsVisible, windowSetSize } from "../lib/window/bus.svelte";
  import { LABEL } from "../lib/window/identity";
  import TitleStrip from "./TitleStrip.svelte";

  // ── skin loading: same pick as the player (first installed, else mock) ──

  let skin = $state<SkinPayload | null>(null);
  let scale = $state(1); // Alt+S toggles 1×/2× (same as the player window)

  $effect(() => {
    let cancelled = false;
    loadDefaultSkin()
      .then((payload) => {
        if (!cancelled) skin = payload;
      })
      .catch((e) => {
        console.warn(`[navinamp] skin load failed (${e}; using mock)`);
        if (!cancelled) skin = mockSkinPayload();
      });
    return () => {
      cancelled = true;
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
    auto: false,
  });

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

  $effect(() => {
    eqPresets()
      .then((p) => (presets = p))
      .catch(() => (presets = []));
  });

  function cyclePreset(): void {
    if (presets.length === 0) return;
    presetIndex = (presetIndex + 1) % presets.length;
    const bands = normalize(presets[presetIndex]?.bandsDb);
    eq.bandsDb = bands;
    bands.forEach((db, i) => eqSetBand(i, db).catch(warnEq));
  }

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
        eq.auto = !eq.auto; // visual only — no backend AUTO yet
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
