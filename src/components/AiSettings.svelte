<script lang="ts">
  /**
   * AI search settings modal. Owns only configuration: provider shape,
   * endpoint/model/key, timeout, and the Discogs token (required for catalog
   * lookups). The AI search prompt lives in the library bar; this panel never
   * sends a search.
   *
   * Keys are stored plaintext in `<app_config>/ai.json` (same policy as
   * navidrome.json) and are never logged. TEST sends one tiny completion
   * with the unsaved draft so endpoint/model/key mistakes surface here.
   */
  import { friendlyIpcError } from "../lib/library/ipc";
  import {
    AI_DEFAULTS,
    AI_PRESETS,
    aiGetConfig,
    aiModels,
    aiSetConfig,
    aiTest,
    presetById,
    type AiConfig,
    type AiProviderKind,
  } from "../lib/ai/ipc";

  interface Props {
    onclose: () => void;
    /** Settings were saved (the library bar can re-read availability). */
    onchanged?: () => void;
  }

  let { onclose, onchanged }: Props = $props();

  let config = $state<AiConfig>({ ...AI_DEFAULTS });
  let loading = $state(true);
  let saving = $state(false);
  let testing = $state(false);
  let note = $state<string | null>(null);
  let error = $state<string | null>(null);
  /** Model ids returned by the provider's live `/models` endpoint. */
  let fetchedModels = $state.raw<string[]>([]);
  /** The current model is not one of the known ids → show the free-text row. */
  let useCustomModel = $state(false);
  let modelsBusy = $state(false);

  const preset = $derived(presetById(config.preset));

  /** Curated preset models plus anything the provider just reported. */
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
        error = null;
      } catch (e) {
        error = friendlyIpcError("AI settings", e);
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

  function chooseProvider(kind: AiProviderKind): void {
    config.provider = kind;
    config.preset = "custom";
    fetchedModels = [];
    note = null;
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
    if (modelsBusy) return;
    modelsBusy = true;
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
      modelsBusy = false;
    }
  }

  async function save(): Promise<void> {
    if (saving) return;
    saving = true;
    note = null;
    error = null;
    try {
      config = await aiSetConfig(config);
      note = "saved";
      onchanged?.();
    } catch (e) {
      error = friendlyIpcError("save AI settings", e);
    } finally {
      saving = false;
    }
  }

  async function test(): Promise<void> {
    if (testing) return;
    testing = true;
    note = null;
    error = null;
    try {
      const reply = await aiTest(config);
      note = `provider replied: ${reply || "(empty)"}`;
    } catch (e) {
      error = friendlyIpcError("test AI provider", e);
    } finally {
      testing = false;
    }
  }
</script>

<div class="overlay" role="dialog" aria-modal="true" aria-label="AI search settings">
  <div class="panel">
    <header class="phdr">AI SEARCH SETTINGS</header>

    <div class="pbody">
      {#if loading}
        <div class="dim">loading…</div>
      {:else}
        <label class="enable">
          <input type="checkbox" bind:checked={config.enabled} />
          enable AI search
          <span class="hint">the AI mode appears in the library search bar</span>
        </label>

        <div class="block">
          <div class="bh">PROVIDER</div>
          <label class="row">
            preset
            <select value={config.preset} onchange={(e) => choosePreset(e.currentTarget.value)}>
              {#each AI_PRESETS as p (p.id)}
                <option value={p.id}>{p.label}</option>
              {/each}
            </select>
          </label>
          {#if preset?.hint}<div class="hint">{preset.hint}</div>{/if}

          <label class="row">
            API shape
            <select
              value={config.provider}
              onchange={(e) => chooseProvider(e.currentTarget.value as AiProviderKind)}>
              <option value="openAiCompatible">OpenAI-compatible (/chat/completions)</option>
              <option value="anthropic">Anthropic (/v1/messages)</option>
            </select>
          </label>

          <label class="row">
            base URL
            <input
              class="txt wide"
              type="text"
              bind:value={config.baseUrl}
              placeholder="https://openrouter.ai/api/v1"
              spellcheck="false" />
          </label>

          <div class="row">
            model
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
              <input
                class="txt wide"
                type="text"
                bind:value={config.model}
                placeholder="model id"
                spellcheck="false" />
            {/if}
            <button
              class="fetch"
              onclick={() => void fetchModels()}
              disabled={modelsBusy}
              title="ask the provider for its /models list">
              {modelsBusy ? "…" : "REFRESH"}
            </button>
          </div>
          {#if modelOptions.length > 0 && useCustomModel}
            <div class="row">
              custom model
              <input
                class="txt wide"
                type="text"
                bind:value={config.model}
                placeholder="model id"
                spellcheck="false" />
            </div>
          {/if}
          <div class="hint">
            REFRESH asks the provider for its model list; the id stays editable.
          </div>

          <label class="row">
            API key
            <input
              class="txt wide"
              type="password"
              bind:value={config.apiKey}
              placeholder="sk-… (blank for local servers)"
              autocomplete="off"
              spellcheck="false" />
          </label>

          <label class="row">
            timeout
            <input class="txt num" type="number" min="5" max="300" bind:value={config.timeoutSecs} />
            <span class="unit">sec</span>
          </label>
        </div>

        <div class="block">
          <div class="bh">DISCOGS CATALOG</div>
          <label class="row">
            token
            <input
              class="txt wide"
              type="password"
              bind:value={config.discogsToken}
              placeholder="required for catalog lookups — free token"
              autocomplete="off"
              spellcheck="false" />
          </label>
          <div class="hint">
            required to list labels, discographies and pressings from Discogs.
            create a free personal access token at discogs.com/settings/developers.
            without it the AI can still build local-track rules, but no catalog is
            returned.
          </div>
        </div>

        <div class="hint plain">
          stored in plaintext at <code>&lt;app_config&gt;/ai.json</code>, like the
          Navidrome credentials. Keys are never sent to the model or written to logs.
        </div>

        {#if note}<div class="note ok">{note}</div>{/if}
        {#if error}<div class="note err">{error}</div>{/if}
      {/if}
    </div>

    <footer class="pbar">
      <button onclick={() => void test()} disabled={loading || testing || saving}>
        {testing ? "TESTING…" : "TEST"}
      </button>
      <button class="primary" onclick={() => void save()} disabled={loading || saving || testing}>
        {saving ? "SAVING…" : "SAVE"}
      </button>
      <button onclick={onclose}>CLOSE</button>
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
    width: min(500px, calc(100% - 16px));
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
    padding: 8px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    font-size: 11px;
    color: #9a9a9a;
    scrollbar-width: thin;
    scrollbar-color: #2f4f33 #000;
  }

  .block {
    display: flex;
    flex-direction: column;
    gap: 5px;
    padding: 6px;
    border: 1px solid #262626;
    background: #111;
  }

  .bh {
    font-size: 10px;
    letter-spacing: 0.12em;
    color: #707070;
  }

  .enable {
    display: flex;
    align-items: center;
    gap: 6px;
    color: #c8c8c8;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .row select {
    flex: 1;
    min-width: 0;
  }

  .txt,
  .row select {
    background: #000;
    color: #3ccb4e;
    border: 1px solid #3a3a3a;
    border-radius: 2px;
    padding: 2px 5px;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 10px;
  }

  .txt:focus,
  .row select:focus {
    outline: none;
    border-color: #3ccb4e;
  }

  .txt {
    flex: 1;
    min-width: 0;
  }

  .txt.wide {
    flex: 1;
  }

  .txt.num {
    flex: none;
    width: 7ch;
  }

  .unit,
  .hint,
  .dim {
    color: #707070;
    font-size: 10px;
  }

  .hint {
    line-height: 1.35;
  }

  .plain {
    padding: 0 2px;
  }

  .plain code {
    color: #8dff9e;
  }

  .pbar {
    display: flex;
    justify-content: flex-end;
    gap: 6px;
    padding: 6px 8px;
    background: #101010;
    border-top: 1px solid #2a2a2a;
  }

  button {
    background: #1c1c1c;
    color: #9a9a9a;
    border: 1px solid #3a3a3a;
    border-radius: 2px;
    padding: 2px 8px;
    font-size: 10px;
    letter-spacing: 0.08em;
    cursor: pointer;
  }

  button:hover:not(:disabled) {
    border-color: #3ccb4e;
    color: #3ccb4e;
  }

  button.primary {
    border-color: #3ccb4e;
    color: #3ccb4e;
  }

  button.fetch {
    flex: none;
    padding: 2px 5px;
    font-size: 9px;
  }

  button:disabled {
    opacity: 0.35;
    cursor: default;
  }

  .note {
    padding: 3px 6px;
    font-size: 10px;
    border: 1px solid #262626;
    background: #0b0b0f;
  }

  .note.ok {
    color: #3ccb4e;
  }

  .note.err {
    color: #d8604c;
  }
</style>
