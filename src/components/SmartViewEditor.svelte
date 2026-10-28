<script lang="ts">
  /**
   * Smart-view rule builder (M4) — Winamp3 "Edit Smart View" flavor.
   *
   * A modal overlay hosted inside the Media Library panel (M6 will move all
   * of this into a real skinned window). The def is edited as raw-string rule
   * rows (`valueRaw`/`value2Raw`) so half-typed numbers never crash the
   * preview; `buildDef()` coerces to the frozen contract shapes on demand:
   * the LIVE preview re-runs it debounced (300 ms) and skips invalid states,
   * SAVE validates strictly and maps failures to inline row errors.
   *
   * Backend validation errors are parsed for a rule index (`rules[1]`,
   * "rule 2", "index #0") and shown under the offending row; anything
   * unparseable degrades to a banner. Missing backend commands surface as a
   * friendly "not available in this build" line — never a crash.
   */
  import {
    friendlyIpcError,
    smartViewDelete,
    smartViewPreview,
    smartViewSave,
    type RuleRow,
    type SmartField,
    type SmartOp,
    type SmartSortField,
    type SmartViewDef,
    type Track,
  } from "../lib/library/ipc";
  import {
    SMART_FIELDS,
    SMART_SORTS,
    defaultOpFor,
    fieldLabel,
    fieldTypeOf,
    inputKindOf,
    opsForField,
  } from "../lib/library/smartView";

  interface Props {
    /** null = creating a new view. */
    id: number | null;
    initial: SmartViewDef;
    onsave: (id: number) => void;
    ondelete: (id: number) => void;
    oncancel: () => void;
  }

  let { id, initial, onsave, ondelete, oncancel }: Props = $props();

  // The editing session snapshots the incoming def once (the parent passes a
  // fresh clone per open); all edits below happen on our own copy.
  // svelte-ignore state_referenced_locally
  const init = $state.snapshot(initial);

  /** Editable rule row: values stay raw strings until buildDef coerces. */
  interface EditableRule {
    field: SmartField;
    op: SmartOp;
    valueRaw: string;
    value2Raw: string;
  }

  let name = $state(init.name);
  let matchAll = $state(init.group.match === "all");
  let rules = $state<EditableRule[]>(
    init.group.rules.map((r) => ({
      field: r.field,
      op: r.op,
      valueRaw: r.value == null ? "" : String(r.value),
      value2Raw: r.value2 == null ? "" : String(r.value2),
    })),
  );
  let limitRaw = $state(init.limit == null ? "" : String(init.limit));
  let sort = $state<SmartSortField>(init.sort);
  let sortDesc = $state(init.sortDesc);

  let rowErrors = $state<Record<number, string>>({});
  let formError = $state<string | null>(null);

  let previewTracks = $state.raw<Track[] | null>(null);
  let previewError = $state<string | null>(null);
  let previewSkipped = $state(false);
  let previewPending = $state(false);

  let saving = $state(false);
  let deleting = $state(false);

  let previewSeq = 0;

  type Built = { ok: true; def: SmartViewDef } | { ok: false; rowErrors: Record<number, string>; message: string };

  /**
   * Coerce the editable rows into a contract-shaped def. `requireName` is
   * false for the live preview (an unnamed def previews as "Untitled") and
   * true for saving.
   */
  function buildDef(requireName: boolean): Built {
    const errs: Record<number, string> = {};
    const rows: RuleRow[] = [];

    for (let i = 0; i < rules.length; i++) {
      const r = rules[i];
      const label = fieldLabel(r.field);
      const type = fieldTypeOf(r.field);
      if (type === "string") {
        if (r.valueRaw.trim() === "") {
          errs[i] = `${label} needs a value`;
          continue;
        }
        rows.push({ field: r.field, op: r.op, value: r.valueRaw });
      } else if (type === "number") {
        const v = Number(r.valueRaw);
        if (r.valueRaw.trim() === "" || !Number.isFinite(v)) {
          errs[i] = `${label} needs a number`;
          continue;
        }
        if (r.op === "between") {
          const v2 = Number(r.value2Raw);
          if (r.value2Raw.trim() === "" || !Number.isFinite(v2)) {
            errs[i] = "range needs an upper bound";
            continue;
          }
          rows.push({ field: r.field, op: r.op, value: v, value2: v2 });
        } else {
          rows.push({ field: r.field, op: r.op, value: v });
        }
      } else {
        // date field
        if (r.op === "inLastDays" || r.op === "notInLastDays") {
          const v = Number(r.valueRaw);
          if (r.valueRaw.trim() === "" || !Number.isFinite(v) || v < 0) {
            errs[i] = "days needs a non-negative number";
            continue;
          }
          rows.push({ field: r.field, op: r.op, value: v });
        } else {
          if (r.valueRaw === "") {
            errs[i] = "pick a date";
            continue;
          }
          rows.push({ field: r.field, op: r.op, value: r.valueRaw });
        }
      }
    }

    if (Object.keys(errs).length > 0) {
      return { ok: false, rowErrors: errs, message: "fix the highlighted rules" };
    }
    if (rules.length === 0) {
      return { ok: false, rowErrors: {}, message: "add at least one rule" };
    }
    if (requireName && name.trim() === "") {
      return { ok: false, rowErrors: {}, message: "name is required" };
    }

    let limit: number | null = null;
    if (limitRaw.trim() !== "") {
      limit = Number(limitRaw);
      if (!Number.isInteger(limit) || limit < 1) {
        return { ok: false, rowErrors: {}, message: "limit must be a whole number ≥ 1 (blank = none)" };
      }
    }

    return {
      ok: true,
      def: {
        name: name.trim() || "Untitled",
        group: { match: matchAll ? "all" : "any", rules: rows },
        limit,
        sort,
        sortDesc,
      },
    };
  }

  // LIVE preview: rebuild on every relevant edit; the effect cleanup clears
  // the pending timer, which gives us the debounce for free.
  $effect(() => {
    const built = buildDef(false);
    // Once the form is valid again, stale row/banner errors clear themselves.
    if (built.ok && (Object.keys(rowErrors).length > 0 || formError != null)) {
      rowErrors = {};
      formError = null;
    }
    if (!built.ok) {
      previewSkipped = true;
      previewPending = false;
      previewTracks = null;
      previewError = null;
      return;
    }
    previewSkipped = false;
    const def = built.def;
    previewPending = true;
    const timer = setTimeout(() => void runPreview(def), 300);
    return () => clearTimeout(timer);
  });

  async function runPreview(def: SmartViewDef): Promise<void> {
    const seq = ++previewSeq;
    try {
      const rows = await smartViewPreview(def);
      if (seq !== previewSeq) return;
      previewTracks = rows;
      previewError = null;
      previewPending = false;
    } catch (e) {
      if (seq !== previewSeq) return;
      previewTracks = null;
      previewError = friendlyIpcError("smart view preview", e);
      previewPending = false;
    }
  }

  function setField(i: number, field: SmartField): void {
    const r = rules[i];
    r.field = field;
    r.op = defaultOpFor(field); // operator must follow the field's type
    r.valueRaw = "";
    r.value2Raw = "";
  }

  function setOp(i: number, op: SmartOp): void {
    const r = rules[i];
    const before = inputKindOf(r.field, r.op);
    const after = inputKindOf(r.field, op);
    r.op = op;
    if (before !== after) {
      r.valueRaw = "";
      r.value2Raw = "";
    } else if (op !== "between") {
      r.value2Raw = "";
    }
  }

  function addRule(): void {
    rules.push({ field: "artist", op: "contains", valueRaw: "", value2Raw: "" });
  }

  function removeRule(i: number): void {
    rules.splice(i, 1);
    rowErrors = {}; // indexes shifted — revalidate on next build
  }

  /**
   * Best-effort localization of a backend error to a rule row: bracketed
   * indexes are 0-based ("rules[1]"), word forms are tried 1-based first
   * ("rule 2", "index #2"). Returns null when nothing parses → banner.
   */
  function parseRuleIndex(msg: string): number | null {
    const bracket = msg.match(/rules?\s*\[\s*(\d+)\s*\]/i);
    if (bracket) {
      const n = Number(bracket[1]);
      if (n >= 0 && n < rules.length) return n;
    }
    const word = msg.match(/(?:rule|row|index)\s*#?\s*(\d+)/i);
    if (word) {
      const n = Number(word[1]);
      if (n >= 1 && n <= rules.length) return n - 1; // 1-based phrasing
      if (n >= 0 && n < rules.length) return n; // 0-based phrasing
    }
    // Mention of a specific field/op token localizes the row that owns it.
    const lower = msg.toLowerCase();
    if (/\bfield\b|\bop\b|\boperator\b/i.test(lower)) {
      for (let i = 0; i < rules.length; i++) {
        if (lower.includes(rules[i].field.toLowerCase()) || lower.includes(rules[i].op.toLowerCase())) {
          return i;
        }
      }
    }
    return null;
  }

  function applyBackendError(msg: string): void {
    const idx = parseRuleIndex(msg);
    if (idx != null) {
      rowErrors = { ...rowErrors, [idx]: msg };
      formError = null;
    } else {
      formError = msg;
    }
  }

  async function save(): Promise<void> {
    if (saving || deleting) return;
    const built = buildDef(true);
    if (!built.ok) {
      rowErrors = built.rowErrors;
      formError = built.message;
      return;
    }
    rowErrors = {};
    formError = null;
    saving = true;
    try {
      const newId = await smartViewSave(id, built.def);
      onsave(id != null ? id : newId);
    } catch (e) {
      applyBackendError(friendlyIpcError("smart view save", e));
    } finally {
      saving = false;
    }
  }

  async function remove(): Promise<void> {
    if (deleting || saving || id == null) return;
    deleting = true;
    try {
      await smartViewDelete(id);
      ondelete(id);
    } catch (e) {
      applyBackendError(friendlyIpcError("smart view delete", e));
    } finally {
      deleting = false;
    }
  }

  function baseName(path: string): string {
    return path.split(/[\\/]/).pop() ?? path;
  }

  function fmt(secs: number): string {
    const m = Math.floor(secs / 60);
    const s = Math.floor(secs % 60);
    return `${m}:${String(s).padStart(2, "0")}`;
  }
</script>

<div class="overlay" role="dialog" aria-modal="true" aria-label="smart view editor">
  <div class="panel">
    <header class="phdr">{id == null ? "NEW SMART VIEW" : `EDIT SMART VIEW — ${initial.name}`}</header>

    <div class="pbody">
      <label class="namerow">
        name
        <input class="txt" type="text" bind:value={name} placeholder="e.g. top rated rock" />
      </label>

      <div class="matchrow">
        match
        <button class:on={matchAll} onclick={() => (matchAll = true)}>ALL</button>
        <button class:ona={!matchAll} onclick={() => (matchAll = false)}>ANY</button>
        <span class="hint">of the following rules</span>
      </div>

      <div class="rules">
        {#each rules as rule, i}
          <div class="rule">
            <div class="rule-line">
              <select
                class="sel"
                value={rule.field}
                onchange={(e) => setField(i, e.currentTarget.value as SmartField)}
                aria-label="field"
              >
                {#each SMART_FIELDS as f (f.id)}
                  <option value={f.id}>{f.label}</option>
                {/each}
              </select>

              <select
                class="sel"
                value={rule.op}
                onchange={(e) => setOp(i, e.currentTarget.value as SmartOp)}
                aria-label="operator"
              >
                {#each opsForField(rule.field) as o (o.id)}
                  <option value={o.id}>{o.label}</option>
                {/each}
              </select>

              {#if inputKindOf(rule.field, rule.op) === "text"}
                <input class="txt val" type="text" bind:value={rule.valueRaw} placeholder="value" aria-label="value" />
              {:else if inputKindOf(rule.field, rule.op) === "number"}
                <input
                  class="txt val"
                  type="number"
                  value={rule.valueRaw}
                  oninput={(e) => (rules[i].valueRaw = e.currentTarget.value)}
                  aria-label="value"
                />
              {:else if inputKindOf(rule.field, rule.op) === "range"}
                <input
                  class="txt val small"
                  type="number"
                  value={rule.valueRaw}
                  oninput={(e) => (rules[i].valueRaw = e.currentTarget.value)}
                  aria-label="lower bound"
                />
                <span class="and">–</span>
                <input
                  class="txt val small"
                  type="number"
                  value={rule.value2Raw}
                  oninput={(e) => (rules[i].value2Raw = e.currentTarget.value)}
                  aria-label="upper bound"
                />
              {:else}
                <input class="txt val" type="date" bind:value={rule.valueRaw} aria-label="date" />
              {/if}

              <button
                class="rm"
                onclick={() => removeRule(i)}
                disabled={rules.length === 1}
                title="remove rule">✕</button>
            </div>
            {#if rowErrors[i]}
              <div class="rule-err">{rowErrors[i]}</div>
            {/if}
          </div>
        {/each}
        <button class="add" onclick={addRule}>+ add rule</button>
      </div>

      <div class="opts">
        <label>
          limit
          <input
            class="txt num"
            type="number"
            min="1"
            placeholder="none"
            value={limitRaw}
            oninput={(e) => (limitRaw = e.currentTarget.value)} />
        </label>
        <label>
          sort
          <select class="sel" value={sort} onchange={(e) => (sort = e.currentTarget.value as SmartSortField)}>
            {#each SMART_SORTS as s (s.id)}
              <option value={s.id}>{s.label}</option>
            {/each}
          </select>
        </label>
        <button class:desc={sortDesc} onclick={() => (sortDesc = !sortDesc)} title="sort direction">
          {sortDesc ? "↓ descending" : "↑ ascending"}
        </button>
      </div>

      <div class="preview">
        <div class="ph">
          preview
          {#if previewPending}
            <span class="dim">running…</span>
          {:else if previewSkipped}
            <span class="dim">complete the rules to preview</span>
          {:else if previewError}
            <span class="err">{previewError}</span>
          {:else if previewTracks}
            <span class="cnt"
              >{previewTracks.length.toLocaleString()} track{previewTracks.length === 1 ? "" : "s"}</span
            >
          {/if}
        </div>
        {#if previewTracks && previewTracks.length > 0}
          <div class="prows">
            {#each previewTracks.slice(0, 50) as t, i (t.id)}
              <div class="prow" title={t.path}>
                <span class="pi">{i + 1}</span>
                <span class="pt">{t.title || baseName(t.path)}</span>
                <span class="pa">{t.artist}</span>
                <span class="pd">{fmt(t.durationSecs)}</span>
              </div>
            {/each}
          </div>
          {#if previewTracks.length > 50}
            <div class="pmore">… first 50 of {previewTracks.length.toLocaleString()}</div>
          {/if}
        {:else if previewTracks && previewTracks.length === 0}
          <div class="pnone">no tracks match yet</div>
        {/if}
      </div>

      {#if formError}
        <div class="banner">{formError}</div>
      {/if}
    </div>

    <footer class="pbar">
      <button class="primary" onclick={save} disabled={saving || deleting}>
        {saving ? "SAVING…" : "SAVE"}
      </button>
      {#if id != null}
        <button class="danger" onclick={remove} disabled={deleting || saving}>
          {deleting ? "DELETING…" : "DELETE"}
        </button>
      {/if}
      <button onclick={oncancel} disabled={saving || deleting}>CANCEL</button>
    </footer>
  </div>
</div>

<style>
  .overlay {
    position: absolute;
    inset: 0;
    z-index: 20;
    display: grid;
    place-items: center;
    background: rgba(0, 0, 0, 0.55);
  }

  .panel {
    width: min(560px, calc(100% - 16px));
    max-height: calc(100% - 16px);
    display: flex;
    flex-direction: column;
    border: 1px solid #3a3a3a;
    background: #161616;
    user-select: none;
  }

  .phdr {
    padding: 4px 8px;
    font-size: 10px;
    letter-spacing: 0.18em;
    color: #808080;
    background: #101010;
    border-bottom: 1px solid #2a2a2a;
  }

  .pbody {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 8px;
    scrollbar-width: thin;
    scrollbar-color: #2f4f33 #000;
  }

  .namerow,
  .matchrow,
  .opts label {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 10px;
    letter-spacing: 0.08em;
    color: #808080;
    text-transform: uppercase;
  }

  .txt,
  .sel {
    background: #000;
    color: #3ccb4e;
    border: 1px solid #3a3a3a;
    border-radius: 2px;
    padding: 2px 6px;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 11px;
  }

  .txt:focus,
  .sel:focus {
    outline: none;
    border-color: #3ccb4e;
  }

  .namerow .txt {
    flex: 1;
  }

  .matchrow button,
  .opts button,
  .pbar button,
  .add,
  .rm {
    background: #1c1c1c;
    color: #9a9a9a;
    border: 1px solid #3a3a3a;
    border-radius: 2px;
    padding: 1px 6px;
    font-size: 10px;
    letter-spacing: 0.08em;
    cursor: pointer;
  }

  .matchrow button:hover:not(:disabled),
  .opts button:hover:not(:disabled),
  .pbar button:hover:not(:disabled),
  .add:hover,
  .rm:hover:not(:disabled) {
    border-color: #3ccb4e;
    color: #3ccb4e;
  }

  button:disabled {
    opacity: 0.35;
    cursor: default;
  }

  .matchrow .on,
  .matchrow .ona {
    border-color: #3ccb4e;
    color: #3ccb4e;
  }

  .matchrow .hint,
  .opts {
    font-size: 10px;
    color: #4f4f4f;
  }

  .rules {
    display: flex;
    flex-direction: column;
    gap: 4px;
    border: 1px solid #2a2a2a;
    background: #101010;
    padding: 6px;
  }

  .rule-line {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .rule-line .sel {
    flex: none;
    max-width: 12ch;
  }

  .val {
    flex: 1;
    min-width: 0;
  }

  .val.small {
    width: 7ch;
    flex: none;
  }

  .and {
    color: #4f4f4f;
  }

  .rm {
    flex: none;
    padding: 1px 4px;
  }

  .add {
    align-self: flex-start;
  }

  .rule-err {
    margin: 1px 0 2px 0;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 10px;
    color: #d8604c;
  }

  .opts {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .opts .num {
    width: 8ch;
  }

  .opts .desc {
    border-color: #3ccb4e;
    color: #3ccb4e;
  }

  .preview {
    border: 1px solid #2a2a2a;
    background: #101010;
    display: flex;
    flex-direction: column;
    min-height: 60px;
  }

  .ph {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 3px 6px;
    font-size: 10px;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: #707070;
    border-bottom: 1px solid #2a2a2a;
  }

  .ph .dim {
    text-transform: none;
    letter-spacing: 0.02em;
    color: #4f4f4f;
  }

  .ph .err {
    text-transform: none;
    letter-spacing: 0.02em;
    color: #d8604c;
  }

  .ph .cnt {
    text-transform: none;
    letter-spacing: 0.02em;
    color: #3ccb4e;
  }

  .prows {
    max-height: 140px;
    overflow-y: auto;
    scrollbar-width: thin;
    scrollbar-color: #2f4f33 #000;
  }

  .prow {
    display: grid;
    grid-template-columns: 4ch 1.4fr 1fr 5ch;
    gap: 6px;
    align-items: center;
    height: 18px;
    line-height: 18px;
    padding: 0 6px;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 11px;
    color: #3ccb4e;
    content-visibility: auto;
    contain-intrinsic-size: auto 18px;
    white-space: nowrap;
    overflow: hidden;
  }

  .pi {
    opacity: 0.6;
  }

  .pt,
  .pa {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .pa {
    color: #2f8f3f;
  }

  .pd {
    text-align: right;
    opacity: 0.8;
  }

  .pnone,
  .pmore {
    padding: 4px 6px;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 10px;
    color: #4f4f4f;
  }

  .banner {
    padding: 4px 6px;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 11px;
    color: #d8604c;
    border: 1px solid #5a2a22;
    background: #180d0b;
  }

  .pbar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 8px;
    background: #101010;
    border-top: 1px solid #2a2a2a;
  }

  .pbar .primary {
    color: #3ccb4e;
  }

  .pbar .danger:hover:not(:disabled) {
    border-color: #d8604c;
    color: #d8604c;
  }
</style>
