<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";

  let appVersion = $state("…");

  $effect(() => {
    void invoke<string>("app_version").then((v) => {
      appVersion = v;
    });
  });
</script>

<main>
  <h1>NavAmp M1 — debug UI</h1>
  <p class="version">app_version: {appVersion}</p>
</main>

<style>
  :global(html),
  :global(body) {
    margin: 0;
    padding: 0;
    background: #000;
  }

  main {
    height: 100vh;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.75rem;
    background: #000;
    color: #c8c8c8;
    font-family: "Segoe UI", "Helvetica Neue", Helvetica, Arial, sans-serif;
    user-select: none;
  }

  h1 {
    margin: 0;
    font-size: 1.5rem;
    font-weight: 600;
    letter-spacing: 0.04em;
    color: #3ccb4e; /* classic Winamp green */
  }

  .version {
    margin: 0;
    font-size: 0.85rem;
    color: #808080;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
  }
</style>
