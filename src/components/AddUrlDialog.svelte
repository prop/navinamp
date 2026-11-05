<script lang="ts">
  /**
   * Small ADD URL dialog for the playlist editor. Direct http(s) audio URLs
   * are added as-is; YouTube links are resolved through the optional yt-dlp
   * helper so the row gets the real title/uploader/duration. The canonical
   * link is what gets queued — the direct media URL is re-resolved on every
   * play (they expire).
   *
   * The dialog doubles as the yt-dlp settings surface: discovery covers PATH,
   * Homebrew and common local installs, with an explicit override for unusual
   * setups. Nothing is bundled; YouTube support is opt-in by installing the
   * helper.
   */
  import { onMount } from "svelte";

  import type { QueueItemMeta } from "../lib/player/queue.svelte";
  import { registerEscape } from "../lib/window/escape";
  import {
    streamsParseUrl,
    streamsResolve,
    streamsSetYtdlpPath,
    streamsYtdlpStatus,
    type YtdlpStatus,
  } from "../lib/streams/ipc";

  interface Props {
    /** Validated row metadata; the parent closes the dialog after adding it. */
    onsubmit: (meta: QueueItemMeta) => void;
    onclose: () => void;
  }

  let { onsubmit, onclose }: Props = $props();

  let url = $state("");
  let busy = $state(false);
  let error = $state<string | null>(null);
  let inputEl = $state<HTMLInputElement | null>(null);

  let status = $state.raw<YtdlpStatus | null>(null);
  let statusError = $state<string | null>(null);
  let showPath = $state(false);
  let pathInput = $state("");
  let savingPath = $state(false);

  $effect(() =>
    registerEscape(() => {
      if (!busy) onclose();
    }),
  );

  const looksYouTube = $derived(/youtu\.?be|youtube\.com/i.test(url));

  function errorText(e: unknown): string {
    return e instanceof Error ? e.message : String(e);
  }

  async function refreshStatus(): Promise<void> {
    try {
      const next = await streamsYtdlpStatus();
      status = next;
      statusError = null;
      if (!showPath) pathInput = next.configuredPath ?? next.resolvedPath ?? "";
    } catch (e) {
      status = null;
      statusError = errorText(e);
    }
  }

  onMount(() => {
    void refreshStatus();
    inputEl?.focus();
  });

  async function submit(): Promise<void> {
    if (busy) return;
    error = null;
    busy = true;
    try {
      const parsed = await streamsParseUrl(url);
      if (parsed.youtube) {
        const resolved = await streamsResolve(parsed.url);
        onsubmit({
          path: parsed.url,
          title: resolved.title || parsed.title,
          artist: resolved.artist || parsed.artist,
          durationSecs: resolved.durationSecs,
          streamKind: "youtube",
        });
      } else {
        onsubmit({
          path: parsed.url,
          title: parsed.title,
          artist: parsed.artist,
          durationSecs: null,
        });
      }
    } catch (e) {
      error = errorText(e);
      // A failed YouTube add is usually the helper: open the path editor.
      if (looksYouTube) showPath = true;
    } finally {
      busy = false;
    }
  }

  function togglePath(): void {
    showPath = !showPath;
    pathInput = status?.configuredPath ?? status?.resolvedPath ?? "";
    error = null;
  }

  async function savePath(): Promise<void> {
    if (savingPath) return;
    savingPath = true;
    error = null;
    try {
      status = await streamsSetYtdlpPath(pathInput.trim() || null);
      showPath = false;
    } catch (e) {
      error = errorText(e);
    } finally {
      savingPath = false;
    }
  }
</script>

<div class="overlay" role="dialog" aria-modal="true" aria-label="Add URL">
  <div class="panel">
    <header class="phdr">
      <span>ADD URL</span>
      <button class="x" onclick={onclose} disabled={busy} title="close">×</button>
    </header>

    <div class="body">
      <input
        bind:this={inputEl}
        bind:value={url}
        class="txt"
        type="text"
        placeholder="https://…"
        spellcheck="false"
        autocapitalize="off"
        autocorrect="off"
        onkeydown={(e) => {
          if (e.key === "Enter") void submit();
        }} />
      <div class="hint">
        Direct http(s) audio streams play as-is. YouTube links play audio-only
        through yt-dlp.
      </div>

      {#if looksYouTube || status?.found !== true || showPath}
        <div class="yt">
          <div class="ytrow">
            <span class="ytlabel">YT-DLP</span>
            {#if status == null && statusError == null}
              <span class="bad">checking…</span>
            {:else if status?.found}
              <span class="ok" title={status.resolvedPath ?? ""}>
                ✓ {status.version ?? "found"}
              </span>
            {:else}
              <span class="bad">not found</span>
            {/if}
            <button class="mini" onclick={togglePath} disabled={savingPath}>
              {showPath ? "HIDE" : "PATH"}
            </button>
          </div>

          {#if showPath}
            <div class="pathrow">
              <input
                bind:value={pathInput}
                class="txt small"
                type="text"
                placeholder="absolute path (blank = auto-detect)"
                spellcheck="false"
                autocapitalize="off"
                autocorrect="off"
                onkeydown={(e) => {
                  if (e.key === "Enter") void savePath();
                }} />
              <button class="mini" onclick={() => void savePath()} disabled={savingPath}>
                {savingPath ? "…" : "SAVE"}
              </button>
            </div>
            <div class="hint">
              e.g. <code>brew install yt-dlp</code>, or save the binary path.
              YouTube support is optional.
            </div>
          {:else if status == null && statusError == null}
            <div class="hint">checking for yt-dlp…</div>
          {:else if status?.found !== true}
            <div class="hint">
              Install <code>yt-dlp</code> (or set its path) to add YouTube links.
            </div>
          {/if}
          {#if statusError}<div class="err small">{statusError}</div>{/if}
        </div>
      {/if}

      {#if error}<div class="err">{error}</div>{/if}
    </div>

    <footer class="acts">
      <button class="primary" onclick={() => void submit()} disabled={busy || url.trim() === ""}>
        {busy ? "RESOLVING…" : "ADD"}
      </button>
      <button onclick={onclose} disabled={busy}>CANCEL</button>
    </footer>
  </div>
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    z-index: 50;
    display: grid;
    place-items: center;
    background: rgba(0, 0, 0, 0.6);
    font-family: Tahoma, "Segoe UI", Geneva, Verdana, sans-serif;
  }

  .panel {
    width: min(259px, calc(100% - 12px));
    max-height: calc(100% - 12px);
    display: flex;
    flex-direction: column;
    border: 1px solid #3a3a3a;
    background: #161616;
    color: #9a9a9a;
    font-size: 11px;
    user-select: none;
  }

  .phdr {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 7px;
    background: #101010;
    border-bottom: 1px solid #2a2a2a;
    color: #808080;
    font-size: 10px;
    letter-spacing: 0.16em;
  }

  .phdr span {
    flex: 1;
  }

  .x {
    border-color: transparent;
    background: transparent;
    color: #707070;
    padding: 0 4px;
  }

  .body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 7px;
    scrollbar-width: thin;
    scrollbar-color: #2f4f33 #000;
  }

  .txt {
    width: 100%;
    box-sizing: border-box;
    background: #0b0b0b;
    color: #d0d0d0;
    border: 1px solid #3a3a3a;
    border-radius: 2px;
    padding: 3px 5px;
    font-family: "SF Mono", Menlo, Consolas, monospace;
    font-size: 10px;
  }

  .txt:focus {
    outline: none;
    border-color: #3ccb4e;
  }

  .txt.small {
    padding: 2px 4px;
    font-size: 9px;
  }

  .hint {
    color: #707070;
    font-size: 9px;
    line-height: 1.35;
  }

  .hint code {
    color: #9a9a9a;
    font-family: "SF Mono", Menlo, Consolas, monospace;
  }

  .yt {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 5px;
    border: 1px solid #262626;
    background: #111;
  }

  .ytrow,
  .pathrow {
    display: flex;
    align-items: center;
    gap: 5px;
  }

  .ytlabel {
    color: #707070;
    font-size: 9px;
    letter-spacing: 0.12em;
  }

  .ok,
  .bad {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    font-size: 9px;
  }

  .ok {
    color: #3ccb4e;
  }

  .bad {
    color: #c75a5a;
  }

  .mini {
    flex: none;
    padding: 1px 5px;
    font-size: 8px;
    letter-spacing: 0.06em;
  }

  .pathrow .txt {
    flex: 1;
    min-width: 0;
  }

  .err {
    color: #d06a6a;
    font-size: 10px;
    line-height: 1.35;
    overflow-wrap: anywhere;
  }

  .err.small {
    font-size: 9px;
  }

  .acts {
    display: flex;
    justify-content: flex-end;
    gap: 5px;
    padding: 6px 7px;
    background: #101010;
    border-top: 1px solid #2a2a2a;
  }

  button {
    background: #1c1c1c;
    color: #9a9a9a;
    border: 1px solid #3a3a3a;
    border-radius: 2px;
    padding: 2px 8px;
    font-family: inherit;
    font-size: 10px;
    letter-spacing: 0.04em;
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

  button.primary {
    color: #d8d8d8;
  }
</style>
