<script lang="ts">
  /**
   * Inline Advanced Search builder: a compact, reusable rule editor that
   * coerces to the same Smart View contract the saved-view editor uses.
   * It owns the draft rows; the parent owns execution (smartViewQuery) and
   * the save-to-view / save-to-playlist actions.
   */
  import {
    type SmartField,
    type SmartOp,
    type SmartSortField,
    type SmartViewDef,
  } from "../lib/library/ipc";
  import {
    SMART_FIELDS,
    SMART_SORTS,
    blankRule,
    coerceRuleRows,
    defaultOpFor,
    inputKindOf,
    normalizeOp,
    opsForField,
    type EditableRule,
  } from "../lib/library/smartView";

  interface Props {
    initial: SmartViewDef;
    busy: boolean;
    onrun: (def: SmartViewDef) => void;
  }

  let { initial, busy, onrun }: Props = $props();

  // The parent remounts this component when it seeds new rules (AI/edit).
  // svelte-ignore state_referenced_locally
  const init = $state.snapshot(initial);
  let matchAll = $state(init.group.match === "all");
  let rules = $state<EditableRule[]>(
    init.group.rules.length > 0
      ? init.group.rules.map((r) => ({
          field: r.field,
          op: normalizeOp(r.field, r.op),
          valueRaw: r.value == null ? "" : String(r.value),
          value2Raw: r.value2 == null ? "" : String(r.value2),
        }))
      : [blankRule()],
  );
  let limitRaw = $state(init.limit == null ? "" : String(init.limit));
  let sort = $state<SmartSortField>(init.sort);
  let sortDesc = $state(init.sortDesc);
  let rowErrors = $state<Record<number, string>>({});
  let formError = $state<string | null>(null);

  function setField(i: number, field: SmartField): void {
    const r = rules[i];
    r.field = field;
    r.op = defaultOpFor(field);
    r.valueRaw = inputKindOf(field, r.op) === "boolean" ? "1" : "";
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
    rules.push(blankRule());
  }

  function removeRule(i: number): void {
    rules.splice(i, 1);
    if (rules.length === 0) rules.push(blankRule());
    rowErrors = {};
  }

  function run(): void {
    const coerced = coerceRuleRows(rules);
    if (!coerced.ok) {
      rowErrors = coerced.errors;
      formError = "fix the highlighted rules";
      return;
    }
    if (coerced.rows.length === 0) {
      formError = "add at least one rule";
      return;
    }
    let limit = limitRaw.trim() === "" ? null : Number(limitRaw);
    if (limit != null && (!Number.isInteger(limit) || limit < 1 || limit > 5000)) {
      formError = "limit must be a whole number between 1 and 5000 (blank = all)";
      return;
    }
    rowErrors = {};
    formError = null;
    onrun({
      name: init.name,
      group: { match: matchAll ? "all" : "any", rules: coerced.rows },
      limit,
      sort,
      sortDesc,
    });
  }
</script>

<div class="adv">
  <div class="advrow">
    <div class="match" role="group" aria-label="match mode">
      <button class:on={matchAll} onclick={() => (matchAll = true)}>ALL</button>
      <button class:on={!matchAll} onclick={() => (matchAll = false)}>ANY</button>
    </div>
    <span class="hint">of these rules</span>
    <select class="sel" bind:value={sort} aria-label="sort">
      {#each SMART_SORTS as s (s.id)}
        <option value={s.id}>{s.label}</option>
      {/each}
    </select>
    <button class="dir" onclick={() => (sortDesc = !sortDesc)} title="sort direction">
      {sortDesc ? "↓" : "↑"}
    </button>
    <label class="lim">
      limit
      <input
        class="txt num"
        type="number"
        min="1"
        max="5000"
        placeholder="all"
        value={limitRaw}
        oninput={(e) => (limitRaw = e.currentTarget.value)} />
    </label>
    <button class="run" onclick={run} disabled={busy}>{busy ? "SEARCHING…" : "RUN"}</button>
  </div>

  <div class="rules">
    {#each rules as rule, i}
      <div class="rulewrap">
        <div class="rule">
          <select
            class="sel"
            value={rule.field}
            onchange={(e) => setField(i, e.currentTarget.value as SmartField)}
            aria-label="field">
            {#each SMART_FIELDS as f (f.id)}
              <option value={f.id}>{f.label}</option>
            {/each}
          </select>
          <select
            class="sel"
            value={rule.op}
            onchange={(e) => setOp(i, e.currentTarget.value as SmartOp)}
            aria-label="operator">
            {#each opsForField(rule.field) as o (o.id)}
              <option value={o.id}>{o.label}</option>
            {/each}
          </select>

          {#if inputKindOf(rule.field, rule.op) === "text"}
            <input
              class="txt val"
              type="text"
              bind:value={rule.valueRaw}
              placeholder="value"
              aria-label="value" />
          {:else if inputKindOf(rule.field, rule.op) === "number"}
            <input
              class="txt val"
              type="number"
              value={rule.valueRaw}
              oninput={(e) => (rules[i].valueRaw = e.currentTarget.value)}
              aria-label="value" />
          {:else if inputKindOf(rule.field, rule.op) === "range"}
            <input
              class="txt val small"
              type="number"
              value={rule.valueRaw}
              oninput={(e) => (rules[i].valueRaw = e.currentTarget.value)}
              aria-label="lower bound" />
            <span class="and">–</span>
            <input
              class="txt val small"
              type="number"
              value={rule.value2Raw}
              oninput={(e) => (rules[i].value2Raw = e.currentTarget.value)}
              aria-label="upper bound" />
          {:else if inputKindOf(rule.field, rule.op) === "boolean"}
            <select
              class="sel"
              value={rule.valueRaw}
              onchange={(e) => (rules[i].valueRaw = e.currentTarget.value)}
              aria-label="value">
              <option value="1">true</option>
              <option value="0">false</option>
            </select>
          {:else}
            <input class="txt val" type="date" bind:value={rule.valueRaw} aria-label="date" />
          {/if}

          <button class="rm" onclick={() => removeRule(i)} title="remove rule">✕</button>
        </div>
        {#if rowErrors[i]}<div class="rerr">{rowErrors[i]}</div>{/if}
      </div>
    {/each}
    <button class="add" onclick={addRule}>+ add rule</button>
  </div>
  {#if formError}<div class="adv-err">{formError}</div>{/if}
</div>

<style>
  .adv {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 5px 6px;
    background: #0b0b0d;
    border-bottom: 1px solid #2a2a2a;
  }

  .advrow {
    display: flex;
    align-items: center;
    gap: 5px;
    flex-wrap: wrap;
    font-size: 10px;
    color: #707070;
  }

  .match {
    display: flex;
    gap: 2px;
  }

  .match button,
  .dir,
  .run,
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

  .match button:hover,
  .dir:hover,
  .run:hover:not(:disabled),
  .add:hover,
  .rm:hover {
    border-color: #3ccb4e;
    color: #3ccb4e;
  }

  .match button.on {
    border-color: #3ccb4e;
    color: #3ccb4e;
    background: #12301b;
  }

  .hint {
    color: #4f4f4f;
  }

  .lim {
    display: flex;
    align-items: center;
    gap: 3px;
  }

  .run {
    margin-left: auto;
    padding: 1px 10px;
    color: #3ccb4e;
  }

  .rules {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .rule {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .txt,
  .sel {
    background: #000;
    color: #3ccb4e;
    border: 1px solid #3a3a3a;
    border-radius: 2px;
    padding: 1px 4px;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 10px;
  }

  .txt:focus,
  .sel:focus {
    outline: none;
    border-color: #3ccb4e;
  }

  .txt.val {
    flex: 1;
    min-width: 60px;
  }

  .txt.num {
    width: 7ch;
  }

  .txt.small {
    flex: 0 1 7ch;
    min-width: 5ch;
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

  .rerr,
  .adv-err {
    font-size: 10px;
    color: #d8604c;
    padding-left: 2px;
  }

  button:disabled {
    opacity: 0.35;
    cursor: default;
  }
</style>
