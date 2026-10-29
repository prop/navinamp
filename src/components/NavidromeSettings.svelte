<script lang="ts">
  /**
   * Navidrome connection + streaming settings (M5) — SmartViewEditor-flavored
   * modal hosted inside the Media Library panel (M6 moves it into a real
   * skinned window).
   *
   * The panel owns its status snapshot: it loads `navidrome_status` on
   * mount, re-reads it whenever `navidrome-synced` fires, and reports its
   * own mutations (connect/disconnect/set-streaming) through `onchanged`
   * so the Media Library's SERVER STATUS node can refresh in step.
   *
   * The password field is `type="password"`, cleared right after a
   * successful connect, and never appears in any log line. Auth failures
   * from `navidrome_connect` (the backend's ping error) show inline; a
   * missing backend command degrades via `friendlyIpcError` — never a
   * crash.
   */
  import { friendlyIpcError } from "../lib/library/ipc";
  import {
    navidromeConnect,
    navidromeDisconnect,
    navidromeSetStreaming,
    navidromeStatus,
    onNavidromeSynced,
    type NavidromeStatus,
    type StreamFormat,
  } from "../lib/navidrome/ipc";

  interface Props {
    onclose: () => void;
    /** Notify the host that status changed (connect/disconnect/prefs). */
    onchanged?: () => void;
    onlog?: (line: string) => void;
  }

  let { onclose, onchanged, onlog }: Props = $props();

  let status = $state.raw<NavidromeStatus | null>(null);
  let statusError = $state<string | null>(null);

  let url = $state("");
  let user = $state("");
  let password = $state("");
  let formError = $state<string | null>(null);
  let connecting = $state(false);
  let disconnecting = $state(false);

  let formatSel = $state<StreamFormat>("original");
  let bitrateRaw = $state("");
  let prefsError = $state<string | null>(null);
  let savingPrefs = $state(false);
  /** Hydrate the streaming inputs from status exactly once (not on every refresh). */
  let prefsHydrated = false;

  let statusSeq = 0;

  async function refreshStatus(): Promise<void> {
    const seq = ++statusSeq;
    try {
      const s = await navidromeStatus();
      if (seq !== statusSeq) return;
      status = s;
      statusError = null;
      // Prefill the form only where the user has not typed anything yet.
      if (url === "") url = s.url ?? "";
      if (user === "") user = s.user ?? "";
      if (!prefsHydrated) {
        prefsHydrated = true;
        formatSel = s.streaming.format;
        bitrateRaw = s.streaming.bitrateKbps == null ? "" : String(s.streaming.bitrateKbps);
      }
    } catch (e) {
      if (seq !== statusSeq) return;
      status = null;
      statusError = friendlyIpcError("navidrome", e);
    }
  }

  $effect(() => {
    void refreshStatus();
    // A sync (usually started from the SERVER STATUS node) updates
    // syncedAt/trackCount/serverVersion — keep the snapshot current.
    const us = onNavidromeSynced(() => void refreshStatus());
    return () => {
      void us.then((f) => f());
    };
  });

  async function connect(): Promise<void> {
    if (connecting || disconnecting) return;
    if (url.trim() === "" || user.trim() === "" || password === "") {
      formError = "server URL, user and password are all required";
      return;
    }
    formError = null;
    connecting = true;
    try {
      status = await navidromeConnect(url.trim(), user.trim(), password);
      password = ""; // never kept around after a successful connect
      onchanged?.();
      onlog?.(
        `navidrome: connected${status.serverVersion ? ` (server ${status.serverVersion})` : ""}`,
      );
    } catch (e) {
      // Auth failures arrive as the backend's ping error — shown inline.
      formError = friendlyIpcError("navidrome connect", e);
    } finally {
      connecting = false;
    }
  }

  async function disconnect(): Promise<void> {
    if (disconnecting || connecting || status?.configured !== true) return;
    disconnecting = true;
    try {
      status = await navidromeDisconnect();
      formError = null;
      onchanged?.();
      onlog?.("navidrome: disconnected");
    } catch (e) {
      formError = friendlyIpcError("navidrome disconnect", e);
    } finally {
      disconnecting = false;
    }
  }

  async function applyPrefs(): Promise<void> {
    if (savingPrefs || status?.configured !== true) return;
    let bitrate: number | null = null;
    if (formatSel !== "original" && bitrateRaw.trim() !== "") {
      const v = Number(bitrateRaw);
      if (!Number.isInteger(v) || v < 1 || v > 3200) {
        prefsError = "bitrate must be a whole number of kbps (1–3200; blank = server default)";
        return;
      }
      bitrate = v;
    }
    prefsError = null;
    savingPrefs = true;
    try {
      status = await navidromeSetStreaming(formatSel, bitrate);
      onchanged?.();
      onlog?.(`navidrome: streaming ${formatSel}${bitrate != null ? ` @ ${bitrate} kbps` : ""}`);
    } catch (e) {
      prefsError = friendlyIpcError("streaming settings", e);
    } finally {
      savingPrefs = false;
    }
  }

  function fmtDate(secs: number | null | undefined): string {
    if (secs == null) return "never";
    return new Date(secs * 1000).toLocaleString();
  }

  const configured = $derived(status?.configured === true);
</script>

<div class="overlay" role="dialog" aria-modal="true" aria-label="navidrome settings">
  <div class="panel">
    <header class="phdr">NAVIDROME SETTINGS</header>

    <div class="pbody">
      <div class="block">
        <div class="bh">SERVER</div>
        {#if statusError}
          <div class="errline">{statusError}</div>
        {:else if status == null}
          <div class="dim">loading…</div>
        {:else if !status.configured}
          <div class="dim">not connected — enter a server URL and credentials below</div>
        {:else}
          <div class="state ok">● CONNECTED</div>
          <div class="kv"><span>url</span><b>{status.url}</b></div>
          <div class="kv"><span>user</span><b>{status.user}</b></div>
          <div class="kv"><span>server version</span><b>{status.serverVersion ?? "—"}</b></div>
          <div class="kv"><span>tracks synced</span><b>{status.trackCount.toLocaleString()}</b></div>
          <div class="kv"><span>last sync</span><b>{fmtDate(status.syncedAt)}</b></div>
        {/if}
      </div>

      <div class="block">
        <div class="bh">CONNECTION</div>
        <label class="field">
          url
          <input class="txt" type="text" placeholder="https://music.example.com" bind:value={url} />
        </label>
        <label class="field">
          user
          <input class="txt" type="text" autocomplete="off" bind:value={user} />
        </label>
        <label class="field">
          password
          <input
            class="txt"
            type="password"
            placeholder={configured ? "(re-enter to reconnect)" : ""}
            autocomplete="new-password"
            bind:value={password} />
        </label>
        <div class="btnrow">
          <button class="primary" onclick={connect} disabled={connecting || disconnecting}>
            {connecting ? "CONNECTING…" : "CONNECT"}
          </button>
          <button onclick={disconnect} disabled={!configured || connecting || disconnecting}>
            {disconnecting ? "DISCONNECTING…" : "DISCONNECT"}
          </button>
        </div>
        {#if formError}
          <div class="banner">{formError}</div>
        {/if}
      </div>

      <div class="block">
        <div class="bh">STREAMING</div>
        <div class="btnrow">
          <label class="field">
            format
            <select
              class="sel"
              disabled={!configured}
              value={formatSel}
              onchange={(e) => (formatSel = e.currentTarget.value as StreamFormat)}>
              <option value="original">original</option>
              <option value="mp3">mp3</option>
              <option value="opus">opus</option>
            </select>
          </label>
          <label class="field">
            bitrate (kbps)
            <input
              class="txt num"
              type="number"
              min="1"
              max="3200"
              placeholder="server default"
              disabled={!configured || formatSel === "original"}
              value={bitrateRaw}
              oninput={(e) => (bitrateRaw = e.currentTarget.value)} />
          </label>
          <button onclick={applyPrefs} disabled={!configured || savingPrefs}>
            {savingPrefs ? "APPLYING…" : "APPLY"}
          </button>
        </div>
        <div class="dim">
          {#if formatSel === "original"}
            original streams the file as-is (no transcode, full quality)
          {:else}
            transcodes are produced by the server on the fly
          {/if}
        </div>
        {#if prefsError}
          <div class="banner">{prefsError}</div>
        {/if}
      </div>
    </div>

    <footer class="pbar">
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
    width: min(460px, calc(100% - 16px));
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
    gap: 10px;
    padding: 8px;
    scrollbar-width: thin;
    scrollbar-color: #2f4f33 #000;
  }

  .block {
    display: flex;
    flex-direction: column;
    gap: 5px;
    border: 1px solid #2a2a2a;
    background: #101010;
    padding: 6px;
  }

  .bh {
    font-size: 10px;
    letter-spacing: 0.12em;
    color: #707070;
  }

  .kv {
    display: grid;
    grid-template-columns: 13ch 1fr;
    gap: 6px;
    align-items: baseline;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 11px;
    color: #3ccb4e;
  }

  .kv span {
    color: #4f4f4f;
  }

  .kv b {
    font-weight: 400;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .state {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 11px;
    letter-spacing: 0.1em;
    color: #808080;
  }

  .state.ok {
    color: #3ccb4e;
  }

  .dim {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 10px;
    color: #4f4f4f;
  }

  .errline {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 11px;
    color: #d8604c;
  }

  .field {
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
    flex: 1;
    min-width: 0;
    background: #000;
    color: #3ccb4e;
    border: 1px solid #3a3a3a;
    border-radius: 2px;
    padding: 2px 6px;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 11px;
    text-transform: none;
    letter-spacing: 0.02em;
  }

  .txt:focus,
  .sel:focus {
    outline: none;
    border-color: #3ccb4e;
  }

  .num {
    flex: none;
    width: 11ch;
  }

  .btnrow {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .btnrow button {
    background: #1c1c1c;
    color: #9a9a9a;
    border: 1px solid #3a3a3a;
    border-radius: 2px;
    padding: 1px 6px;
    font-size: 10px;
    letter-spacing: 0.08em;
    cursor: pointer;
  }

  .btnrow button:hover:not(:disabled) {
    border-color: #3ccb4e;
    color: #3ccb4e;
  }

  .btnrow button:disabled {
    opacity: 0.35;
    cursor: default;
  }

  .btnrow .primary {
    color: #3ccb4e;
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

  .pbar button {
    background: #1c1c1c;
    color: #9a9a9a;
    border: 1px solid #3a3a3a;
    border-radius: 2px;
    padding: 1px 6px;
    font-size: 10px;
    letter-spacing: 0.08em;
    cursor: pointer;
  }

  .pbar button:hover {
    border-color: #3ccb4e;
    color: #3ccb4e;
  }
</style>
