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
   *
   * With `manageCurves` (library window), a collapsible AUTO CURVES manager
   * lists saved track/artist associations, applies one to the live bands and
   * deletes individual entries; the EQ window's fallback stays compact.
   */
  import {
    EQ_BAND_COUNT,
    eqDeletePreset,
    eqHeadroom,
    eqPresets,
    eqSavePreset,
    eqSetBand,
    eqSetEnabled,
    eqSetPreamp,
    eqState,
    notifyPresetsChanged,
    type EqPreset,
  } from "../lib/audio/eq";
  import {
    listEqAssociations,
    onEqAssocChanged,
    removeEqArtist,
    removeEqTrack,
    type EqArtistEntry,
    type EqAssociationList,
    type EqTrackEntry,
  } from "../lib/audio/auto";
  import { friendlyIpcError, libraryTrack } from "../lib/library/ipc";
  import { untrack } from "svelte";

  // Classic Winamp EQ center frequencies (Hz) and their panel labels.
  const BAND_HZ = [60, 170, 310, 600, 1000, 3000, 6000, 12000, 14000, 16000];
  const BAND_LABELS = ["60", "170", "310", "600", "1k", "3k", "6k", "12k", "14k", "16k"];
  const DB_MIN = -12;
  const DB_MAX = 12;
  const DB_RANGE = DB_MAX - DB_MIN;

  interface Props {
    /** Show the saved track/artist AUTO curve manager (library window only). */
    manageCurves?: boolean;
  }

  let { manageCurves = false }: Props = $props();

  let enabled = $state(false);
  let preampDb = $state(0);
  let bandsDb = $state<number[]>(Array<number>(EQ_BAND_COUNT).fill(0));
  let presets = $state<EqPreset[]>([]);
  let presetName = $state("");
  let newPresetName = $state("");
  let presetNote = $state<string | null>(null);
  let presetBusy = $state(false);
  let available = $state(false);
  let error = $state<string | null>(null);
  /** Auto attenuation readout in dB (0 or negative); null until queried. */
  let headroomDb = $state<number | null>(null);
  let headroomSeq = 0;

  // ── AUTO CURVES manager (library window only) ────────────────────────────
  // Reads the same localStorage associations the context menus write. Track
  // titles were not stored on older entries, so they are resolved lazily via
  // `libraryTrack` when the manager is open; the basename is the fallback.
  let curvesOpen = $state(false);
  let associations = $state.raw<EqAssociationList>({ tracks: [], artists: [] });
  let curveMeta = $state.raw<Record<string, { title: string; artist: string }>>({});
  let curveNote = $state<string | null>(null);
  let curveError = $state<string | null>(null);
  let curveBusy = $state(false);

  const resolvingPaths = new Set<string>();

  const trackCurveCount = $derived(associations.tracks.length);
  const artistCurveCount = $derived(associations.artists.length);
  const curveCount = $derived(trackCurveCount + artistCurveCount);

  const selectedIsUser = $derived(presets.find((p) => p.name === presetName)?.user === true);

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
      void syncHeadroom();
    } catch (e) {
      note(e);
    }
  }

  /**
   * Refresh the auto-headroom readout. A sequence guard keeps a slow reply
   * from overwriting a newer curve's value; a missing command hides the line.
   */
  async function syncHeadroom(bands: number[] = bandsDb): Promise<void> {
    const seq = ++headroomSeq;
    try {
      const db = await eqHeadroom(bands);
      if (seq === headroomSeq) headroomDb = Number.isFinite(db) ? db : null;
    } catch {
      if (seq === headroomSeq) headroomDb = null;
    }
  }

  // Bootstrap once on mount: current state + the preset list.
  $effect(() => {
    void sync();
    eqPresets()
      .then((p) => (presets = p))
      .catch(() => (presets = [])); // missing presets just empty the dropdown
  });

  // ── AUTO CURVES manager ──────────────────────────────────────────────────

  function refreshCurves(): void {
    associations = listEqAssociations();
  }

  // Keep the list live while the library panel is mounted: saves/clears from
  // either window's context menus broadcast the same event.
  $effect(() => {
    if (!manageCurves) return;
    refreshCurves();
    const unlisten = onEqAssocChanged(() => refreshCurves()).catch(
      () => () => undefined,
    );
    return () => {
      void unlisten.then((f) => f());
    };
  });

  /** Resolve missing titles for older entries, a few lookups at a time. */
  async function resolveTitles(): Promise<void> {
    const missing = associations.tracks
      .filter(
        (e) =>
          e.title.trim() === "" &&
          curveMeta[e.path] == null &&
          !resolvingPaths.has(e.path),
      )
      .map((e) => e.path)
      .slice(0, 100);
    if (missing.length === 0) return;
    for (const path of missing) resolvingPaths.add(path);
    let cursor = 0;
    const worker = async (): Promise<void> => {
      while (cursor < missing.length) {
        const path = missing[cursor++];
        if (path == null) return;
        try {
          const track = await libraryTrack(path);
          if (track != null) {
            curveMeta = {
              ...curveMeta,
              [path]: { title: track.title, artist: track.artist },
            };
          }
        } catch {
          // unknown path: keep the basename fallback
        } finally {
          resolvingPaths.delete(path);
        }
      }
    };
    await Promise.all(
      Array.from({ length: Math.min(6, missing.length) }, () => worker()),
    );
  }

  $effect(() => {
    if (!manageCurves || !curvesOpen) return;
    void associations; // retry after a save/delete changed the list
    untrack(() => {
      void resolveTitles();
    });
  });

  function pathBasename(path: string): string {
    const clean = path.split(/[?#]/, 1)[0] ?? path;
    const base = clean.split(/[\\/]/).pop() ?? path;
    try {
      return decodeURIComponent(base) || path;
    } catch {
      return base;
    }
  }

  function trackLabel(entry: EqTrackEntry): string {
    const info = curveMeta[entry.path];
    const title = entry.title.trim() || info?.title.trim() || "";
    const artist = entry.artist.trim() || info?.artist.trim() || "";
    if (title !== "" && artist !== "") return `${artist} — ${title}`;
    if (title !== "") return title;
    if (artist !== "") return `${artist} — ${pathBasename(entry.path)}`;
    return pathBasename(entry.path);
  }

  /** Tiny 10-bar preview: positive bands above the midline, cuts below. */
  function barHeight(db: number): number {
    return Math.max(1, Math.round((Math.abs(db) / DB_MAX) * 5));
  }

  function barTop(db: number): number {
    const h = barHeight(db);
    return db >= 0 ? 6 - h : 6;
  }

  function applySavedCurve(bands: number[]): void {
    if (!available) return;
    const normalized = normalize(bands);
    bandsDb = normalized;
    presetName = ""; // this is an association, not a preset
    normalized.forEach((db, i) => eqSetBand(i, db).catch(note));
    if (!enabled) {
      enabled = true;
      eqSetEnabled(true).catch(note);
    }
    curveError = null;
    curveNote = "applied to the live EQ";
    void syncHeadroom();
  }

  async function deleteTrackCurve(entry: EqTrackEntry): Promise<void> {
    if (curveBusy) return;
    const label = trackLabel(entry);
    curveBusy = true;
    curveNote = null;
    curveError = null;
    try {
      const changed = await removeEqTrack(entry.path);
      if (changed) curveNote = `deleted the track curve for “${label}”`;
      refreshCurves();
    } catch (e) {
      curveError = friendlyIpcError("delete EQ curve", e);
    } finally {
      curveBusy = false;
    }
  }

  async function deleteArtistCurve(entry: EqArtistEntry): Promise<void> {
    if (curveBusy) return;
    curveBusy = true;
    curveNote = null;
    curveError = null;
    try {
      const changed = await removeEqArtist(entry.key);
      if (changed) curveNote = `deleted the artist curve for ${entry.artist}`;
      refreshCurves();
    } catch (e) {
      curveError = friendlyIpcError("delete EQ curve", e);
    } finally {
      curveBusy = false;
    }
  }

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
    presetNote = null;
    const preset = presets.find((p) => p.name === name);
    if (!preset) return;
    const bands = normalize(preset.bandsDb);
    bandsDb = bands; // preamp deliberately untouched by presets
    bands.forEach((db, i) => eqSetBand(i, db).catch(note));
    // Applying a preset implies wanting to HEAR it (the EQ ships bypassed).
    if (!enabled) {
      enabled = true;
      eqSetEnabled(true).catch(note);
    }
    void syncHeadroom();
  }

  async function savePreset(): Promise<void> {
    const name = newPresetName.trim();
    if (name === "" || !available || presetBusy) return;
    presetBusy = true;
    try {
      presets = await eqSavePreset(name, bandsDb);
      presetName = name;
      newPresetName = "";
      presetNote = `saved “${name}”`;
      error = null;
      void notifyPresetsChanged();
    } catch (e) {
      note(e);
    } finally {
      presetBusy = false;
    }
  }

  async function deletePreset(): Promise<void> {
    if (!selectedIsUser || presetBusy) return;
    const name = presetName;
    presetBusy = true;
    try {
      presets = await eqDeletePreset(name);
      presetNote = `deleted “${name}”`;
      presetName = "";
      error = null;
      void notifyPresetsChanged();
    } catch (e) {
      note(e);
    } finally {
      presetBusy = false;
    }
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
      void syncHeadroom();
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
          <option value={p.name}>{p.name}{p.user ? " *" : ""}</option>
        {/each}
      </select>
    </label>
  </div>

  <div class="row savebar">
    <input
      class="pname"
      type="text"
      bind:value={newPresetName}
      placeholder="save current as…"
      maxlength="40"
      aria-label="new preset name"
      onkeydown={(e) => {
        if (e.key === "Enter") void savePreset();
      }} />
    <button
      onclick={() => void savePreset()}
      disabled={!available || presetBusy || newPresetName.trim() === ""}>save</button>
    {#if selectedIsUser}
      <button class="del" onclick={() => void deletePreset()} disabled={presetBusy}>delete</button>
    {/if}
    {#if presetNote}<span class="note">{presetNote}</span>{/if}
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

  {#if headroomDb != null}
    <div
      class="headroom"
      class:off={!enabled}
      title="automatic attenuation applied before the EQ so boosted curves cannot clip; the volume control sits after it and can raise the result back">
      <span>auto headroom</span>
      <span class="hrval" class:cut={headroomDb < -0.05}>
        {headroomDb > -0.05 ? "0.0" : headroomDb.toFixed(1)} dB
      </span>
    </div>
  {/if}

  {#if manageCurves}
    <div class="curves">
      <button
        class="curvetoggle"
        class:on={curvesOpen}
        onclick={() => (curvesOpen = !curvesOpen)}
        aria-expanded={curvesOpen}>
        <span>auto curves</span>
        <span class="cnt">({curveCount})</span>
        <span class="tw">{curvesOpen ? "▾" : "▸"}</span>
      </button>

      {#if curvesOpen}
        {#if curveError}<div class="err">{curveError}</div>{/if}
        {#if curveNote}<div class="note">{curveNote}</div>{/if}

        {#if curveCount === 0}
          <div class="cempty">
            no saved curves — use a track row's right-click menu to save the live
            EQ curve for that track or artist
          </div>
        {:else}
          {#if trackCurveCount > 0}
            <h3>track curves <span>({trackCurveCount})</span></h3>
            <ul class="curvelist">
              {#each associations.tracks as entry (entry.path)}
                <li>
                  <span class="cmini" aria-hidden="true">
                    {#each entry.bandsDb as db, i}
                      <i
                        class:up={db >= 0}
                        style="left:{i * 5}px; top:{barTop(db)}px; height:{barHeight(db)}px"
                      ></i>
                    {/each}
                  </span>
                  <span class="clabel" title={entry.path}>{trackLabel(entry)}</span>
                  <button
                    class="capply"
                    onclick={() => applySavedCurve(entry.bandsDb)}
                    disabled={!available}
                    title="load this curve into the live EQ">apply</button>
                  <button
                    class="cdel"
                    onclick={() => void deleteTrackCurve(entry)}
                    disabled={curveBusy}
                    title="delete this track curve">×</button>
                </li>
              {/each}
            </ul>
          {/if}

          {#if artistCurveCount > 0}
            <h3>artist curves <span>({artistCurveCount})</span></h3>
            <ul class="curvelist">
              {#each associations.artists as entry (entry.key)}
                <li>
                  <span class="cmini" aria-hidden="true">
                    {#each entry.bandsDb as db, i}
                      <i
                        class:up={db >= 0}
                        style="left:{i * 5}px; top:{barTop(db)}px; height:{barHeight(db)}px"
                      ></i>
                    {/each}
                  </span>
                  <span class="clabel" title={entry.artist}>{entry.artist}</span>
                  <button
                    class="capply"
                    onclick={() => applySavedCurve(entry.bandsDb)}
                    disabled={!available}
                    title="load this curve into the live EQ">apply</button>
                  <button
                    class="cdel"
                    onclick={() => void deleteArtistCurve(entry)}
                    disabled={curveBusy}
                    title="delete this artist curve">×</button>
                </li>
              {/each}
            </ul>
          {/if}
        {/if}
      {/if}
    </div>
  {/if}
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

  .savebar {
    gap: 0.5rem;
  }

  .pname {
    width: 15ch;
    background: #1a1a1a;
    color: #c8c8c8;
    border: 1px solid #3a3a3a;
    border-radius: 3px;
    padding: 0.3rem 0.4rem;
    font-size: 0.8rem;
  }

  .savebar button {
    padding: 0.3rem 0.6rem;
    font-size: 0.8rem;
  }

  .savebar .del {
    border-color: #5a2a22;
    color: #d8604c;
  }

  .note {
    font-size: 0.75rem;
    color: #3ccb4e;
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

  .headroom {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    margin-top: -0.35rem;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 10px;
    letter-spacing: 0.06em;
    color: #707070;
  }

  .headroom.off {
    opacity: 0.45;
  }

  .headroom .hrval {
    color: #3ccb4e;
  }

  .headroom .hrval.cut {
    color: #d8a63c;
  }

  /* ── AUTO CURVES manager ── */
  .curves {
    width: 100%;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    padding-top: 0.75rem;
    border-top: 1px solid #222;
  }

  .curvetoggle {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    text-align: left;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    font-size: 0.72rem;
    background: #101014;
  }

  .curvetoggle .cnt {
    color: #707070;
  }

  .curvetoggle .tw {
    margin-left: auto;
    color: #707070;
  }

  .curves h3 {
    width: 100%;
    margin: 0.15rem 0 0;
    display: flex;
    justify-content: space-between;
    font-size: 0.62rem;
    font-weight: 600;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: #707070;
  }

  .curves h3 span {
    color: #4f4f4f;
  }

  .curves > .err,
  .curves > .note {
    width: 100%;
    text-align: center;
  }

  .curvelist {
    width: 100%;
    box-sizing: border-box;
    max-height: 132px;
    overflow-y: auto;
    margin: 0;
    padding: 0;
    list-style: none;
    background: #0b0b0f;
    border: 1px solid #26262e;
    border-radius: 2px;
  }

  .curvelist li {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 2px 5px;
    border-bottom: 1px solid #16161c;
  }

  .curvelist li:last-child {
    border-bottom: 0;
  }

  .clabel {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    font-size: 10px;
    color: #b8b8c0;
  }

  .cmini {
    position: relative;
    flex: none;
    width: 50px;
    height: 12px;
  }

  .cmini i {
    position: absolute;
    width: 4px;
    background: #d8a63c;
    border-radius: 1px;
  }

  .cmini i.up {
    background: #3ccb4e;
  }

  .curvelist button {
    flex: none;
    padding: 0 5px;
    font-size: 9px;
    line-height: 15px;
    border-radius: 2px;
  }

  .curvelist .capply {
    color: #8a8a92;
  }

  .curvelist .cdel {
    border-color: #5a2a22;
    color: #d8604c;
  }

  .curvelist .cdel:hover:not(:disabled) {
    border-color: #d8604c;
    color: #ff7a66;
  }

  .cempty {
    width: 100%;
    box-sizing: border-box;
    padding: 6px;
    background: #0b0b0f;
    border: 1px solid #26262e;
    border-radius: 2px;
    font-size: 10px;
    line-height: 1.4;
    color: #707070;
    text-align: center;
  }
</style>
