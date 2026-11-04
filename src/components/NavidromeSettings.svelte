<script lang="ts">
  /**
   * Navidrome connection + streaming settings (M5) — SmartViewEditor-flavored
   * modal hosted inside the Media Library panel (M6 moves it into a real
   * skinned window).
   *
   * The panel owns its status snapshot: it loads `navidrome_status` on
   * mount, re-reads it whenever `navidrome-synced` fires, and reports its
   * own mutations (connect/switch/remove/set-streaming) through `onchanged`
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
    navidromeLibraries,
    navidromeProfiles,
    navidromeRemoveProfile,
    navidromeRenameProfile,
    navidromeSetLibrary,
    navidromeSetStreaming,
    navidromeStatus,
    navidromeSwitch,
    navidromeSync,
    onNavidromeSynced,
    type MusicFolder,
    type NavidromeProfile,
    type NavidromeStatus,
    type StreamFormat,
  } from "../lib/navidrome/ipc";

  interface Props {
    onclose: () => void;
    /** Notify the host that status changed (connect/switch/prefs). */
    onchanged?: () => void;
    onlog?: (line: string) => void;
  }

  let { onclose, onchanged, onlog }: Props = $props();

  let status = $state.raw<NavidromeStatus | null>(null);
  let statusError = $state<string | null>(null);

  let profiles = $state.raw<NavidromeProfile[]>([]);
  let profilesError = $state<string | null>(null);
  let switchingId = $state<string | null>(null);
  let removingId = $state<string | null>(null);
  let renamingId = $state<string | null>(null);
  let renameValue = $state("");

  let url = $state("");
  let user = $state("");
  let password = $state("");
  /** Accept self-signed / untrusted TLS certificates for this server. */
  let insecureTls = $state(false);
  /** Optional display name for the server being added/updated. */
  let nameInput = $state("");
  let formError = $state<string | null>(null);
  let connecting = $state(false);

  let formatSel = $state<StreamFormat>("original");
  let bitrateRaw = $state("");
  let prefsError = $state<string | null>(null);
  let savingPrefs = $state(false);
  /** Hydrate the streaming inputs from status exactly once (not on every refresh). */
  let prefsHydrated = false;

  let statusSeq = 0;

  function hydratePrefs(s: NavidromeStatus): void {
    formatSel = s.streaming.format;
    bitrateRaw = s.streaming.bitrateKbps == null ? "" : String(s.streaming.bitrateKbps);
    prefsHydrated = true;
  }

  async function refreshStatus(): Promise<void> {
    const seq = ++statusSeq;
    try {
      const s = await navidromeStatus();
      if (seq !== statusSeq) return;
      status = s;
      statusError = null;
      if (!prefsHydrated) hydratePrefs(s);
    } catch (e) {
      if (seq !== statusSeq) return;
      status = null;
      statusError = friendlyIpcError("navidrome", e);
    }
  }

  async function refreshProfiles(): Promise<void> {
    try {
      profiles = await navidromeProfiles();
      profilesError = null;
    } catch (e) {
      profiles = [];
      profilesError = friendlyIpcError("navidrome profiles", e);
    }
  }

  async function refreshAll(): Promise<void> {
    await refreshStatus();
    await refreshProfiles();
  }

  $effect(() => {
    void refreshAll();
    // A sync (usually started from the SERVER STATUS node) updates
    // syncedAt/trackCount/serverVersion — keep the snapshot current.
    const us = onNavidromeSynced(() => void refreshAll());
    return () => {
      void us.then((f) => f());
    };
  });

  function resetForm(): void {
    url = "";
    user = "";
    password = "";
    insecureTls = false;
    nameInput = "";
    formError = null;
  }

  /** Saved profile matching the typed url+user (same normalization as the
   *  backend: trimmed user, trailing slashes dropped from the url). */
  const formMatch = $derived.by((): NavidromeProfile | null => {
    const candidateUrl = url.trim().replace(/\/+$/, "");
    const candidateUser = user.trim();
    if (candidateUrl === "" || candidateUser === "") return null;
    return (
      profiles.find(
        (p) => p.url.replace(/\/+$/, "") === candidateUrl && p.user === candidateUser,
      ) ?? null
    );
  });

  const formTitle = $derived(
    formMatch == null ? "+ ADD SERVER" : formMatch.active ? "RECONNECT" : "UPDATE SERVER",
  );

  const formCta = $derived(
    connecting
      ? "CONNECTING…"
      : formMatch == null
        ? "ADD & CONNECT"
        : formMatch.active
          ? "RECONNECT"
          : "UPDATE & RECONNECT",
  );

  const formHint = $derived(
    formMatch == null
      ? "adds the server, syncs it and makes it active — the other servers stay visible"
      : formMatch.active
        ? "reconnects to the active server with these credentials"
        : "updates this saved server and makes it active",
  );

  async function connect(): Promise<void> {
    if (connecting) return;
    if (url.trim() === "" || user.trim() === "" || password === "") {
      formError = "server URL, user and password are all required";
      return;
    }
    formError = null;
    connecting = true;
    try {
      const previousProfile = status?.profileId ?? null;
      status = await navidromeConnect(
        url.trim(),
        user.trim(),
        password,
        nameInput.trim() === "" ? null : nameInput.trim(),
        insecureTls,
      );
      resetForm(); // the next add starts clean, never from the last server
      if (status.profileId !== previousProfile) hydratePrefs(status);
      await refreshProfiles();
      onchanged?.();
      onlog?.(
        `navidrome: connected to ${status.profileLabel ?? status.url}` +
          `${status.serverVersion ? ` (server ${status.serverVersion})` : ""}`,
      );
    } catch (e) {
      // Auth failures arrive as the backend's ping error — shown inline.
      formError = friendlyIpcError("navidrome connect", e);
    } finally {
      connecting = false;
    }
  }

  async function switchProfile(id: string): Promise<void> {
    if (switchingId || removingId || connecting) return;
    const profile = profiles.find((p) => p.id === id);
    switchingId = id;
    formError = null;
    try {
      status = await navidromeSwitch(id);
      resetForm();
      hydratePrefs(status);
      await refreshProfiles();
      onchanged?.();
      onlog?.(`navidrome: switched to ${profile?.name ?? id}`);
    } catch (e) {
      formError = friendlyIpcError("server switch", e);
    } finally {
      switchingId = null;
    }
  }

  async function removeProfile(profile: NavidromeProfile): Promise<void> {
    if (removingId || switchingId || connecting) return;
    const inForm = formMatch?.id === profile.id;
    const question = profile.active
      ? `Remove “${profile.name}” and delete its synced tracks?`
      : `Remove the saved server “${profile.name}”?`;
    if (!window.confirm(question)) return;
    removingId = profile.id;
    formError = null;
    try {
      status = await navidromeRemoveProfile(profile.id);
      if (renamingId === profile.id) renamingId = null;
      if (profile.active) prefsHydrated = false;
      await refreshProfiles();
      // The removed server's credentials must not linger in the add form.
      if (inForm || profile.active) resetForm();
      onchanged?.();
      onlog?.(`navidrome: removed ${profile.name}`);
    } catch (e) {
      formError = friendlyIpcError("server removal", e);
    } finally {
      removingId = null;
    }
  }

  function startRename(profile: NavidromeProfile): void {
    renamingId = profile.id;
    renameValue = profile.label ?? "";
    formError = null;
  }

  async function applyRename(): Promise<void> {
    if (renamingId == null) return;
    const id = renamingId;
    const label = renameValue.trim() === "" ? null : renameValue.trim();
    try {
      status = await navidromeRenameProfile(id, label);
      renamingId = null;
      await refreshProfiles();
      onchanged?.();
      onlog?.(`navidrome: renamed server to ${label ?? "(automatic)"}`);
    } catch (e) {
      formError = friendlyIpcError("server rename", e);
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

  // ── library (music folder) selection ─────────────────────────────────────
  // Navidrome exposes each library through getMusicFolders; the selected id
  // scopes the sync walk (musicFolderId). Applying triggers a sync, whose
  // prune removes remote rows outside the new selection.

  let libraries = $state.raw<MusicFolder[]>([]);
  let libraryId = $state("");
  let libError = $state<string | null>(null);
  let libLoading = $state(false);
  let libSaving = $state(false);
  let libSeq = 0;

  const libraryName = $derived(
    status?.libraryId == null
      ? "all libraries"
      : (libraries.find((l) => l.id === status?.libraryId)?.name ?? status.libraryId),
  );

  async function refreshLibraries(): Promise<void> {
    if (status?.configured !== true) {
      libraries = [];
      libraryId = "";
      libLoading = false;
      return;
    }
    const seq = ++libSeq;
    libLoading = true;
    try {
      const list = await navidromeLibraries();
      if (seq !== libSeq) return;
      libraries = list;
      libraryId = status.libraryId ?? "";
      libError = null;
    } catch (e) {
      if (seq !== libSeq) return;
      libraries = [];
      libError = friendlyIpcError("navidrome libraries", e);
    } finally {
      if (seq === libSeq) libLoading = false;
    }
  }

  $effect(() => {
    void status?.configured;
    void status?.profileId;
    void status?.libraryId;
    void refreshLibraries();
  });

  async function applyLibrary(): Promise<void> {
    if (!configured || libSaving) return;
    libSaving = true;
    libError = null;
    try {
      status = await navidromeSetLibrary(libraryId === "" ? null : libraryId);
      onchanged?.();
      const name =
        libraryId === ""
          ? "all libraries"
          : (libraries.find((l) => l.id === libraryId)?.name ?? libraryId);
      onlog?.(`navidrome: library — ${name}`);
      // Re-sync just the active server after changing its library scope.
      void navidromeSync(status.profileId, false).catch((e) => {
        const msg = e instanceof Error ? e.message : String(e);
        if (!/already|running/i.test(msg)) onlog?.(`navidrome: sync error: ${msg}`);
      });
    } catch (e) {
      libError = friendlyIpcError("library selection", e);
    } finally {
      libSaving = false;
    }
  }
</script>

<div class="overlay" role="dialog" aria-modal="true" aria-label="navidrome settings">
  <div class="panel">
    <header class="phdr">NAVIDROME SETTINGS</header>

    <div class="pbody">
      <div class="block">
        <div class="bh">SERVERS</div>
        {#if profilesError}
          <div class="errline">{profilesError}</div>
        {:else if profiles.length === 0}
          <div class="dim">no saved servers yet — add your first below</div>
        {:else}
          {#each profiles as profile (profile.id)}
            <div class="prow" class:active={profile.active}>
              {#if renamingId === profile.id}
                <input
                  class="txt"
                  type="text"
                  placeholder={`${profile.user}@…`}
                  bind:value={renameValue}
                  onkeydown={(e) => {
                    if (e.key === "Enter") void applyRename();
                    if (e.key === "Escape") renamingId = null;
                  }} />
                <div class="pactions">
                  <button onclick={() => void applyRename()}>SAVE</button>
                  <button onclick={() => (renamingId = null)}>CANCEL</button>
                </div>
              {:else}
                <div class="pinfo">
                  <b>{profile.name}</b>
                  {#if profile.active}<span class="pactive">● ACTIVE</span>{/if}
                  <span class="purl">
                    {profile.user} · {profile.url} · last sync {fmtDate(profile.syncedAt)}
                  </span>
                </div>
                <div class="pactions">
                  {#if !profile.active}
                    <button
                      onclick={() => void switchProfile(profile.id)}
                      disabled={switchingId != null || connecting}>
                      {switchingId === profile.id ? "SWITCHING…" : "SWITCH"}
                    </button>
                  {/if}
                  <button onclick={() => startRename(profile)} disabled={removingId != null}>
                    RENAME
                  </button>
                  <button
                    onclick={() => void removeProfile(profile)}
                    disabled={removingId != null}>
                    {removingId === profile.id ? "REMOVING…" : "REMOVE"}
                  </button>
                </div>
              {/if}
            </div>
          {/each}
        {/if}
        {#if profiles.length > 1}
          <div class="dim">all saved servers stay synced; the active one drives playlists and publishing</div>
        {/if}

        <div class="add">
          <div class="addhdr">{formTitle}</div>
          <label class="field">
            url
            <input
              class="txt"
              type="text"
              placeholder="https://music.example.com"
              bind:value={url} />
          </label>
          <label class="field">
            name
            <input
              class="txt"
              type="text"
              placeholder="optional — defaults to user@host"
              bind:value={nameInput} />
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
              placeholder={formMatch ? "(re-enter to reconnect)" : ""}
              autocomplete="new-password"
              bind:value={password} />
          </label>
          <label class="check">
            <input type="checkbox" bind:checked={insecureTls} />
            allow self-signed certificates
          </label>
          <div class="btnrow">
            <button class="primary" onclick={connect} disabled={connecting}>{formCta}</button>
          </div>
          <div class="dim">{formHint}</div>
          {#if formError}
            <div class="banner">{formError}</div>
          {/if}
        </div>
      </div>

      <div class="block">
        <div class="bh">SERVER</div>
        {#if statusError}
          <div class="errline">{statusError}</div>
        {:else if status == null}
          <div class="dim">loading…</div>
        {:else if !status.configured}
          <div class="dim">not connected — add or switch to a server above</div>
        {:else}
          <div class="state ok">● CONNECTED</div>
          <div class="kv"><span>profile</span><b>{status.profileLabel ?? status.profileId}</b></div>
          <div class="kv"><span>url</span><b>{status.url}</b></div>
          <div class="kv"><span>user</span><b>{status.user}</b></div>
          <div class="kv"><span>server version</span><b>{status.serverVersion ?? "—"}</b></div>
          <div class="kv"><span>library</span><b>{libraryName}</b></div>
          <div class="kv"><span>tracks synced</span><b>{status.trackCount.toLocaleString()}</b></div>
          <div class="kv"><span>last sync</span><b>{fmtDate(status.syncedAt)}</b></div>
        {/if}
      </div>

      <div class="block">
        <div class="bh">LIBRARY</div>
        {#if !configured}
          <div class="dim">connect first</div>
        {:else if libLoading}
          <div class="dim">loading libraries…</div>
        {:else}
          <div class="btnrow">
            <label class="field">
              library
              <select class="sel" bind:value={libraryId} disabled={libSaving}>
                <option value="">all libraries</option>
                {#each libraries as lib (lib.id)}
                  <option value={lib.id}>{lib.name}</option>
                {/each}
              </select>
            </label>
            <button onclick={() => void applyLibrary()} disabled={libSaving}>
              {libSaving ? "APPLYING…" : "APPLY"}
            </button>
          </div>
          <div class="dim">
            applying re-syncs and removes remote tracks outside the selection
          </div>
          {#if libError}
            <div class="banner">{libError}</div>
          {/if}
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

  .prow {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 3px 4px;
    border: 1px solid #242424;
    background: #0b0b0b;
  }

  .add {
    display: flex;
    flex-direction: column;
    gap: 5px;
    margin-top: 3px;
    padding-top: 6px;
    border-top: 1px solid #242424;
  }

  .addhdr {
    font-size: 10px;
    letter-spacing: 0.12em;
    color: #3ccb4e;
  }

  .prow.active {
    border-color: #2f4f33;
    background: #0d130d;
  }

  .pinfo {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 6px;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 11px;
  }

  .pinfo b {
    color: #3ccb4e;
    font-weight: 400;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .pactive {
    font-size: 9px;
    letter-spacing: 0.1em;
    color: #3ccb4e;
  }

  .purl {
    flex-basis: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 10px;
    color: #4f4f4f;
  }

  .pactions {
    display: flex;
    align-items: center;
    gap: 4px;
    flex: none;
  }

  .pactions button {
    background: #1c1c1c;
    color: #9a9a9a;
    border: 1px solid #3a3a3a;
    border-radius: 2px;
    padding: 1px 5px;
    font-size: 9px;
    letter-spacing: 0.06em;
    cursor: pointer;
  }

  .pactions button:hover:not(:disabled) {
    border-color: #3ccb4e;
    color: #3ccb4e;
  }

  .pactions button:disabled {
    opacity: 0.35;
    cursor: default;
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

  /* TLS opt-in row: checkbox + label inline (not uppercase — it reads as a
     sentence, matching the dim hints). */
  .check {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 10px;
    color: #808080;
    cursor: pointer;
  }

  .check input {
    accent-color: #3ccb4e;
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
