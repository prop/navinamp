<script lang="ts">
  /**
   * Winamp-EQ-styled 10-band equalizer panel (M6): ON/OFF toggle, PRESET
   * dropdown (eq_presets — applying one sets every band and deliberately
   * leaves the preamp), PRE + 10 vertical band sliders (60 Hz … 16 kHz,
   * ±12 dB, integer dB like classic Winamp) and an approximate
   * frequency-response sparkline.
   *
   * IPC contract (frozen M6): eq_state / eq_set_enabled / eq_set_preamp /
   * eq_set_band / eq_presets. State is bootstrapped from eq_state; slider
   * drags update locally and fire eq_set_band fire-and-forget (never one
   * await per tick), and releasing a groove does a trailing eq_state sync so
   * the panel converges with the engine. A missing backend command degrades
   * via friendlyIpcError to an inline note with inert controls.
   */
  import {
    EQ_BAND_COUNT,
    eqPresets,
    eqSetBand,
    eqSetEnabled,
    eqSetPreamp,
    eqState,
    type EqPreset,
  } from "../lib/audio/eq";
  import { friendlyIpcError } from "../lib/library/ipc";

  // Classic Winamp EQ center frequencies (Hz) and their panel labels.
  const BAND_HZ = [60, 170, 310, 600, 1000, 3000, 6000, 12000, 14000, 16000];
  const BAND_LABELS = ["60", "170", "310", "600", "1k", "3k", "6k", "12k", "14k", "16k"];
  const DB_MIN = -12;
  const DB_MAX = 12;
  const DB_RANGE = DB_MAX - DB_MIN;

  let enabled = $state(false);
  let preampDb = $state(0);
  let bandsDb = $state<number[]>(Array<number>(EQ_BAND_COUNT).fill(0));
  let presets = $state<EqPreset[]>([]);
  let presetName = $state("");
  let available = $state(false);
  let error = $state<string | null>(null);

  // Which groove the pointer owns: "pre" or a band index (null = none).
  let dragging: "pre" | number | null = null;

  function note(e: unknown): void {
    error = friendlyIpcError("equalizer", e);
  }

  function normalize(bands: number[] | undefined | null): number[] {
    const out = Array<number>(EQ_BAND_COUNT).fill(0);
    if (Array.isArray(bands)) {
      for (let i = 0; i < EQ_BAND_COUNT; i++) out[i] = bands[i] ?? 0;
    }
    return out;
  }

  /** Trailing reconcile — the engine is the source of truth. */
  async function sync(): Promise<void> {
    try {
      const s = await eqState();
      enabled = s.enabled;
      preampDb = s.preampDb ?? 0;
      bandsDb = normalize(s.bandsDb);
      available = true;
      error = null;
    } catch (e) {
      note(e);
    }
  }

  // Bootstrap once on mount: current state + the preset list.
  $effect(() => {
    void sync();
    eqPresets()
      .then((p) => (presets = p))
      .catch(() => (presets = [])); // missing presets just empty the dropdown
  });

  function toggleEnabled(): void {
    const next = !enabled;
    enabled = next; // optimistic; revert via the trailing sync on error
    eqSetEnabled(next).catch((e) => {
      note(e);
      void sync();
    });
  }

  function applyPreset(name: string): void {
    presetName = name;
    const preset = presets.find((p) => p.name === name);
    if (!preset) return;
    const bands = normalize(preset.bandsDb);
    bandsDb = bands; // preamp deliberately untouched by presets
    bands.forEach((db, i) => eqSetBand(i, db).catch(note));
  }

  function setBand(i: number, db: number): void {
    bandsDb[i] = db; // optimistic; the release-time sync reconciles
    eqSetBand(i, db).catch(note);
  }

  function dbFromPointer(e: PointerEvent & { currentTarget: HTMLElement }): number {
    const r = e.currentTarget.getBoundingClientRect();
    const f = 1 - (e.clientY - r.top) / r.height; // 0 dB in the center
    const db = DB_MIN + Math.min(1, Math.max(0, f)) * DB_RANGE;
    return Math.round(db); // integer dB — the classic Winamp feel
  }

  function applyDrag(e: PointerEvent & { currentTarget: HTMLElement }): void {
    const db = dbFromPointer(e);
    if (dragging === "pre") {
      if (preampDb !== db) {
        preampDb = db;
        eqSetPreamp(db).catch(note);
      }
    } else if (typeof dragging === "number" && bandsDb[dragging] !== db) {
      setBand(dragging, db);
    }
  }

  function grooveDown(e: PointerEvent & { currentTarget: HTMLElement }, kind: "pre" | number): void {
    if (!available) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    dragging = kind;
    applyDrag(e);
  }

  function grooveMove(e: PointerEvent & { currentTarget: HTMLElement }): void {
    if (dragging != null) applyDrag(e);
  }

  function grooveUp(): void {
    if (dragging == null) return;
    dragging = null;
    void sync(); // trailing sync on release — not per drag tick
  }

  function grooveKey(e: KeyboardEvent, kind: "pre" | number): void {
    if (!available) return;
    const cur = kind === "pre" ? preampDb : (bandsDb[kind] ?? 0);
    let db: number | null = null;
    if (e.key === "ArrowUp" || e.key === "ArrowRight") db = Math.min(DB_MAX, cur + 1);
    else if (e.key === "ArrowDown" || e.key === "ArrowLeft") db = Math.max(DB_MIN, cur - 1);
    else if (e.key === "Home") db = DB_MAX;
    else if (e.key === "End") db = DB_MIN;
    if (db == null) return;
    e.preventDefault();
    if (kind === "pre") {
      preampDb = db;
      eqSetPreamp(db).catch(note);
    } else {
      setBand(kind, db);
    }
  }

  // ── frequency-response sparkline (bonus, approximate) ──────────────────────
  // NOT a real biquad cascade: each band contributes one Gaussian in
  // log2-frequency space (~1 octave width — the classic EQ's fixed Q), which
  // sketches the magnitude response well enough for a 40px-tall visual.

  const SPARK_W = 292;
  const SPARK_H = 40;

  let spark: HTMLCanvasElement | null = null;

  function responseDb(f: number): number {
    let sum = 0;
    for (let i = 0; i < EQ_BAND_COUNT; i++) {
      const oct = Math.log2(f / BAND_HZ[i]);
      sum += bandsDb[i] * Math.exp(-(oct * oct) / 0.4);
    }
    return sum;
  }

  $effect(() => {
    void preampDb;
    for (const db of bandsDb) void db; // track every band
    const ctx = spark?.getContext("2d");
    if (!ctx) return;
    ctx.clearRect(0, 0, SPARK_W, SPARK_H);
    const mid = SPARK_H / 2;
    const yFor = (db: number) =>
      mid - (Math.max(DB_MIN, Math.min(DB_MAX, db)) / DB_RANGE) * (SPARK_H - 8);

    // 0 dB midline.
    ctx.strokeStyle = "#2c2c34";
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(0, mid + 0.5);
    ctx.lineTo(SPARK_W, mid + 0.5);
    ctx.stroke();

    // Preamp: a uniform shift, drawn as a dim flat line.
    ctx.strokeStyle = "rgba(216, 166, 60, 0.5)";
    ctx.beginPath();
    ctx.moveTo(0, yFor(preampDb) + 0.5);
    ctx.lineTo(SPARK_W, yFor(preampDb) + 0.5);
    ctx.stroke();

    // Response curve, 20 Hz → 20 kHz on a log-x axis.
    ctx.strokeStyle = "#3ccb4e";
    ctx.beginPath();
    const N = 96;
    for (let i = 0; i <= N; i++) {
      const f = 20 * Math.pow(1000, i / N);
      const x = Math.round((i / N) * (SPARK_W - 1)) + 0.5;
      const y = Math.round(yFor(responseDb(f))) + 0.5;
      if (i === 0) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    }
    ctx.stroke();
  });
</script>

<section class="eq">
  <h2>
    equalizer
    {#if error}<span class="err">{error}</span>{/if}
  </h2>

  <div class="row">
    <button class:on={enabled} onclick={toggleEnabled} disabled={!available}>
      {enabled ? "on" : "off"}
    </button>
    <label>
      preset
      <select
        value={presetName}
        onchange={(e) => applyPreset(e.currentTarget.value)}
        disabled={!available}
      >
        <option value=""></option>
        {#each presets as p (p.name)}
          <option value={p.name}>{p.name}</option>
        {/each}
      </select>
    </label>
  </div>

  <div class="bands" class:bypassed={!enabled}>
    <div class="band pre">
      <div
        class="groove"
        role="slider"
        tabindex="0"
        aria-label="preamp"
        aria-orientation="vertical"
        aria-valuemin={DB_MIN}
        aria-valuemax={DB_MAX}
        aria-valuenow={preampDb}
        title="preamp {preampDb >= 0 ? "+" : ""}{preampDb} dB"
        onpointerdown={(e) => grooveDown(e, "pre")}
        onpointermove={grooveMove}
        onpointerup={grooveUp}
        onpointercancel={grooveUp}
        onkeydown={(e) => grooveKey(e, "pre")}
      >
        <span class="zero"></span>
        <span class="thumb" style="bottom:calc((100% - 8px) * {(preampDb - DB_MIN) / DB_RANGE})"></span>
      </div>
      <span class="lbl">pre</span>
    </div>

    {#each BAND_LABELS as lbl, i (lbl)}
      <div class="band">
        <div
          class="groove"
          role="slider"
          tabindex="0"
          aria-label="{lbl} Hz band"
          aria-orientation="vertical"
          aria-valuemin={DB_MIN}
          aria-valuemax={DB_MAX}
          aria-valuenow={bandsDb[i]}
          title="{BAND_HZ[i]} Hz {bandsDb[i] >= 0 ? "+" : ""}{bandsDb[i]} dB"
          onpointerdown={(e) => grooveDown(e, i)}
          onpointermove={grooveMove}
          onpointerup={grooveUp}
          onpointercancel={grooveUp}
          onkeydown={(e) => grooveKey(e, i)}
        >
          <span class="zero"></span>
          <span class="thumb" style="bottom:calc((100% - 8px) * {(bandsDb[i] - DB_MIN) / DB_RANGE})"></span>
        </div>
        <span class="lbl">{lbl}</span>
      </div>
    {/each}
  </div>

  <canvas
    class="resp"
    bind:this={spark}
    width={SPARK_W}
    height={SPARK_H}
    title="approximate frequency response (band gains only)"
  ></canvas>
</section>

<style>
  .eq {
    width: 100%;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.75rem;
    padding-top: 1.5rem;
    border-top: 1px solid #222;
  }

  .eq h2 {
    margin: 0;
    font-size: 0.8rem;
    font-weight: 600;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: #707070;
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .err {
    font-weight: 400;
    text-transform: none;
    letter-spacing: 0.02em;
    color: #d8604c;
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

  button.on {
    border-color: #3ccb4e;
    color: #3ccb4e;
  }

  label {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.8rem;
    color: #808080;
  }

  select {
    background: #1a1a1a;
    color: #c8c8c8;
    border: 1px solid #3a3a3a;
    border-radius: 3px;
    padding: 0.3rem 0.4rem;
    font-size: 0.8rem;
  }

  select:disabled {
    opacity: 0.4;
  }

  .bands {
    display: flex;
    align-items: flex-end;
    gap: 6px;
  }

  .bands.bypassed {
    opacity: 0.55;
  }

  .band {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    width: 20px;
  }

  .band.pre {
    margin-right: 10px;
  }

  .groove {
    position: relative;
    width: 14px;
    height: 96px;
    background: #101015;
    border: 1px solid #3c3c46;
    border-radius: 2px;
    cursor: ns-resize;
    touch-action: none;
  }

  .groove:focus-visible {
    outline: 1px solid #3ccb4e;
  }

  .zero {
    position: absolute;
    left: 1px;
    right: 1px;
    top: 50%;
    height: 1px;
    background: #3c3c46;
  }

  .thumb {
    position: absolute;
    left: 1px;
    right: 1px;
    height: 8px;
    box-sizing: border-box;
    background: linear-gradient(#9a9aa6, #4c4c58);
    border: 1px solid #c8c8d0;
    border-radius: 1px;
  }

  .lbl {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 9px;
    color: #808080;
    white-space: nowrap;
  }

  .resp {
    image-rendering: pixelated;
    background: #0b0b0f;
    border: 1px solid #26262e;
    border-radius: 2px;
  }
</style>
