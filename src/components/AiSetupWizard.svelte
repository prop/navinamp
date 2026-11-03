<script lang="ts">
  /**
   * Provider setup wizard shown in the onboarding tour. Three steps:
   * provider preset → endpoint/key/model → optional Discogs token + test/save.
   * "Use offline mode" skips without changing anything; the tour then finishes.
   */
  import {
    AI_DEFAULTS,
    AI_PRESETS,
    aiGetConfig,
    aiModels,
    aiSetConfig,
    aiTest,
    notifyAiConfigChanged,
    presetById,
    type AiConfig,
  } from "../lib/ai/ipc";
  import { friendlyIpcError } from "../lib/library/ipc";

  interface Props {
    oncomplete: () => void;
    onskip: () => void;
  }

  let { oncomplete, onskip }: Props = $props();

  let config = $state<AiConfig>({ ...AI_DEFAULTS });
  let step = $state(0);
  let loading = $state(true);
  let busy = $state(false);
  let note = $state<string | null>(null);
  let error = $state<string | null>(null);
  let fetchedModels = $state.raw<string[]>([]);
  let useCustomModel = $state(false);

  const preset = $derived(presetById(config.preset));

  const modelOptions = $derived.by(() => {
    const seen = new Set<string>();
    const out: string[] = [];
    for (const id of [...(preset?.models ?? []), ...fetchedModels]) {
      const clean = id.trim();
      if (clean === "" || seen.has(clean)) continue;
      seen.add(clean);
      out.push(clean);
    }
    return out;
  });

  $effect(() => {
    void (async () => {
      try {
        config = await aiGetConfig();
        useCustomModel = !(presetById(config.preset)?.models ?? []).includes(
          config.model,
        );
      } catch {
        // defaults are usable; the user can still fill the form
      } finally {
        loading = false;
      }
    })();
  });

  function choosePreset(id: string): void {
    const next = presetById(id);
    if (!next) return;
    config.preset = id;
    config.provider = next.provider;
    config.baseUrl = next.baseUrl;
    config.model = next.model;
    fetchedModels = [];
    useCustomModel = next.models.length === 0;
    note = null;
    error = null;
  }

  function chooseModel(value: string): void {
    if (value === "__custom") {
      useCustomModel = true;
      return;
    }
    useCustomModel = false;
    config.model = value;
  }

  async function fetchModels(): Promise<void> {
    if (busy) return;
    busy = true;
    note = null;
    error = null;
    try {
      const list = await aiModels(config);
      fetchedModels = list;
      useCustomModel = !list.includes(config.model);
      note = `fetched ${list.length} model${list.length === 1 ? "" : "s"}`;
    } catch (e) {
      error = friendlyIpcError("fetch models", e);
    } finally {
      busy = false;
    }
  }

  async function test(): Promise<void> {
    if (busy) return;
    busy = true;
    note = null;
    error = null;
    try {
      const reply = await aiTest(config);
      note = `provider replied: ${reply || "(empty)"}`;
    } catch (e) {
      error = friendlyIpcError("test provider", e);
    } finally {
      busy = false;
    }
  }

  async function save(): Promise<void> {
    if (busy) return;
    busy = true;
    note = null;
    error = null;
    config.enabled = true;
    try {
      config = await aiSetConfig(config);
      void notifyAiConfigChanged();
      oncomplete();
    } catch (e) {
      error = friendlyIpcError("save provider", e);
    } finally {
      busy = false;
    }
  }

  function keyHint(): string {
    switch (config.preset) {
      case "openrouter":
        return "openrouter.ai/keys — one key covers many models";
      case "openai":
        return "platform.openai.com/api-keys";
      case "anthropic":
        return "console.anthropic.com → API keys";
      case "deepseek":
        return "platform.deepseek.com/api_keys";
      case "glm":
        return "z.ai → API keys (Z.ai platform)";
      case "kimi":
        return "platform.kimi.ai → API keys";
      case "minimax":
        return "platform.minimax.io → API keys";
      default:
        return "any OpenAI-compatible endpoint; local servers may not need a key";
    }
  }
</script>

<div class="wizard">
  <div class="whead">
    <span class="step">STEP {step + 1}/3</span>
    <span class="dots">
      <i class:on={step >= 0}></i><i class:on={step >= 1}></i><i class:on={step >= 2}></i>
    </span>
  </div>

  {#if loading}
    <div class="dim">loading settings…</div>
  {:else if step === 0}
    <div class="field">
      <div class="label">provider</div>
      <select value={config.preset} onchange={(e) => choosePreset(e.currentTarget.value)}>
        {#each AI_PRESETS as p (p.id)}
          <option value={p.id}>{p.label}</option>
        {/each}
      </select>
      {#if preset?.hint}<div class="hint">{preset.hint}</div>{/if}
      <div class="hint">
        {config.preset === "custom"
          ? "Point the base URL at an OpenAI-compatible endpoint (Ollama, LM Studio, a gateway)."
          : "Model, endpoint and key are editable on the next step."}
      </div>
    </div>
  {:else if step === 1}
    <div class="field">
      <div class="label">API key <span class="req">required for cloud providers</span></div>
      <input
        class="txt"
        type="password"
        bind:value={config.apiKey}
        placeholder="paste the provider key"
        autocomplete="off"
        spellcheck="false" />
      <div class="hint">{keyHint()}</div>
    </div>
    <div class="field">
      <div class="label">base URL</div>
      <input
        class="txt"
        type="text"
        bind:value={config.baseUrl}
        spellcheck="false" />
    </div>
    <div class="field">
      <div class="label">model</div>
      <div class="modelrow">
        {#if modelOptions.length > 0}
          <select
            value={useCustomModel ? "__custom" : config.model}
            onchange={(e) => chooseModel(e.currentTarget.value)}>
            {#each modelOptions as id (id)}
              <option value={id}>{id}</option>
            {/each}
            <option value="__custom">custom…</option>
          </select>
        {:else}
          <input class="txt" type="text" bind:value={config.model} spellcheck="false" />
        {/if}
        <button class="mini" onclick={() => void fetchModels()} disabled={busy}>
          {busy ? "…" : "REFRESH"}
        </button>
      </div>
      {#if modelOptions.length > 0 && useCustomModel}
        <input class="txt" type="text" bind:value={config.model} placeholder="model id" spellcheck="false" />
      {/if}
      <div class="hint">REFRESH asks the provider for its model list.</div>
    </div>
  {:else}
    <div class="field">
      <div class="label">Discogs token <span class="opt">optional</span></div>
      <input
        class="txt"
        type="password"
        bind:value={config.discogsToken}
        placeholder="required for catalog lookups"
        autocomplete="off"
        spellcheck="false" />
      <div class="hint">
        Required to list labels, discographies and pressings. Create a free token at
        discogs.com/settings/developers. Leave it blank for local-only search.
      </div>
    </div>
    <div class="testrow">
      <button onclick={() => void test()} disabled={busy}>{busy ? "TESTING…" : "TEST PROVIDER"}</button>
      <button class="primary" onclick={() => void save()} disabled={busy}>SAVE &amp; FINISH</button>
    </div>
  {/if}

  {#if error}<div class="err">{error}</div>{/if}
  {#if note}<div class="ok">{note}</div>{/if}

  <div class="wfoot">
    <button class="plain" onclick={onskip} disabled={busy}>USE OFFLINE MODE</button>
    <span class="grow"></span>
    {#if step > 0}
      <button onclick={() => (step -= 1)} disabled={busy}>BACK</button>
    {/if}
    {#if step < 2}
      <button class="primary" onclick={() => (step += 1)} disabled={busy}>NEXT</button>
    {/if}
  </div>
</div>

<style>
  .wizard {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 6px;
    border: 1px solid #262626;
    background: #0b0b0f;
  }

  .whead {
    display: flex;
    align-items: center;
    gap: 6px;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 9px;
    color: #707070;
  }

  .step {
    letter-spacing: 0.08em;
  }

  .dots {
    display: flex;
    gap: 3px;
  }

  .dots i {
    width: 16px;
    height: 2px;
    background: #2a2a2a;
  }

  .dots i.on {
    background: #3ccb4e;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .label {
    font-size: 10px;
    color: #9a9a9a;
    text-transform: uppercase;
    letter-spacing: 0.08em;
  }

  .req,
  .opt {
    text-transform: none;
    letter-spacing: 0;
    color: #707070;
    font-size: 9px;
  }

  .txt,
  select {
    background: #000;
    color: #3ccb4e;
    border: 1px solid #3a3a3a;
    border-radius: 2px;
    padding: 2px 5px;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 10px;
  }

  .txt:focus,
  select:focus {
    outline: none;
    border-color: #3ccb4e;
  }

  .txt {
    width: 100%;
    box-sizing: border-box;
  }

  .modelrow {
    display: flex;
    gap: 4px;
  }

  .modelrow select {
    flex: 1;
    min-width: 0;
  }

  .modelrow .txt {
    flex: 1;
  }

  .hint,
  .dim {
    font-size: 9px;
    line-height: 1.35;
    color: #707070;
  }

  button {
    background: #1c1c1c;
    color: #9a9a9a;
    border: 1px solid #3a3a3a;
    border-radius: 2px;
    padding: 2px 8px;
    font-size: 10px;
    letter-spacing: 0.06em;
    cursor: pointer;
  }

  button:hover:not(:disabled) {
    border-color: #3ccb4e;
    color: #3ccb4e;
  }

  button.primary {
    color: #3ccb4e;
    border-color: #3ccb4e;
  }

  button.plain {
    border-color: transparent;
    background: transparent;
    color: #707070;
  }

  button.mini {
    flex: none;
    padding: 2px 5px;
    font-size: 9px;
  }

  button:disabled {
    opacity: 0.35;
    cursor: default;
  }

  .testrow {
    display: flex;
    gap: 6px;
  }

  .wfoot {
    display: flex;
    align-items: center;
    gap: 6px;
    padding-top: 3px;
    border-top: 1px solid #1c1c1c;
  }

  .grow {
    flex: 1;
  }

  .err {
    font-size: 10px;
    color: #d8604c;
  }

  .ok {
    font-size: 10px;
    color: #3ccb4e;
  }
</style>
