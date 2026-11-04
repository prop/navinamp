<script lang="ts">
  /**
   * Onboarding AI demo: runs the deterministic offline rules parser, so the
   * demo needs no provider, key or network. Shows the generated rule text and
   * a few matching tracks, then hands off to the optional provider wizard.
   */
  import {
    friendlyIpcError,
    libraryGenres,
    smartViewQuery,
    type Track,
  } from "../lib/library/ipc";
  import { aiOfflineSearch, type AiSearchReport } from "../lib/ai/ipc";
  import { plainRules } from "../lib/library/smartView";

  interface Props {
    onadvance: () => void;
  }

  let { onadvance }: Props = $props();

  let prompt = $state("jazz from the 70s");
  let busy = $state(false);
  let report = $state.raw<AiSearchReport | null>(null);
  let rows = $state.raw<Track[]>([]);
  let error = $state<string | null>(null);
  let prepared = false;

  async function choosePrompt(): Promise<void> {
    if (prepared) return;
    prepared = true;
    try {
      const genres = await libraryGenres();
      const jazz = genres.find((genre) => /jazz/i.test(genre));
      const picked = jazz ?? genres[0];
      if (picked != null && picked.trim() !== "") {
        prompt = jazz != null ? "jazz from the 70s" : `${picked} from the 90s`;
      }
    } catch {
      // empty library / backend unavailable: the static example still works
    }
  }

  $effect(() => {
    void choosePrompt();
  });

  async function runDemo(): Promise<void> {
    if (busy || prompt.trim() === "") return;
    busy = true;
    error = null;
    report = null;
    rows = [];
    try {
      const result = await aiOfflineSearch(prompt.trim());
      report = result;
      if (result.def != null) {
        rows = await smartViewQuery(result.def, 200);
      }
    } catch (e) {
      error = friendlyIpcError("offline AI demo", e);
    } finally {
      busy = false;
    }
  }

  function trackLabel(track: Track): string {
    const title = track.title.trim() || track.path.split(/[\\/]/).pop() || track.path;
    return track.artist.trim() !== "" ? `${track.artist} — ${title}` : title;
  }
</script>

<div class="demo">
  <div class="badge">offline · no key / no network</div>
  <label class="prompt">
    prompt
    <input
      class="txt"
      type="text"
      bind:value={prompt}
      spellcheck="false"
      autocorrect="off"
      autocapitalize="off"
      autocomplete="off"
      onkeydown={(e) => {
        if (e.key === "Enter") void runDemo();
      }} />
  </label>
  <div class="row">
    <button class="run" onclick={() => void runDemo()} disabled={busy || prompt.trim() === ""}>
      {busy ? "RUNNING…" : "RUN DEMO"}
    </button>
    <span class="hint">try “jazz from the 70s”, “flac from the 90s”, or an artist name</span>
  </div>

  {#if error}
    <div class="err">{error}</div>
  {/if}

  {#if report?.def}
    <div class="result">
      <div class="rules">{plainRules(report.def)}</div>
      <div class="count">
        {rows.length.toLocaleString()} track{rows.length === 1 ? "" : "s"}
        {rows.length >= 200 ? " (first 200)" : ""}
      </div>
      {#if rows.length > 0}
        <ul class="hits">
          {#each rows.slice(0, 5) as track (track.id)}
            <li>{trackLabel(track)}</li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}

  <button class="advance" onclick={onadvance}>NEXT: CONNECT A PROVIDER</button>
</div>

<style>
  .demo {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 6px;
    border: 1px solid #262626;
    background: #0b0b0f;
  }

  .badge {
    align-self: flex-start;
    padding: 0 5px;
    border: 1px solid #2f4f33;
    border-radius: 2px;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 9px;
    color: #3ccb4e;
  }

  .prompt {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 10px;
    color: #808080;
    text-transform: uppercase;
    letter-spacing: 0.08em;
  }

  .txt {
    flex: 1;
    min-width: 0;
    background: #000;
    color: #3ccb4e;
    border: 1px solid #3a3a3a;
    border-radius: 2px;
    padding: 2px 5px;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 11px;
  }

  .txt:focus {
    outline: none;
    border-color: #3ccb4e;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }

  .hint,
  .count {
    font-size: 9px;
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

  button.run {
    color: #3ccb4e;
  }

  button:disabled {
    opacity: 0.35;
    cursor: default;
  }

  .result {
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding: 4px 5px;
    border: 1px solid #1e3a24;
    background: #0b140d;
  }

  .rules {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 10px;
    color: #8dff9e;
    line-height: 1.35;
  }

  .hits {
    margin: 2px 0 0;
    padding-left: 16px;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 9px;
    color: #b8b8c0;
  }

  .err {
    font-size: 10px;
    color: #d8604c;
  }

  .advance {
    align-self: flex-end;
    color: #3ccb4e;
  }
</style>
