<script lang="ts">
  /**
   * Reusable tab strip for the non-skin windows (#16): Winamp-ish dark
   * chrome that sits under a `TitleStrip`, one button per pane. Pure
   * props/callbacks — the caller owns which pane is mounted and any
   * persistence of the active tab.
   *
   * Styling matches the TitleStrip look (Silkscreen caps, 1px dark
   * borders, classic green for the active tab).
   */
  export interface TabItem {
    id: string;
    label: string;
  }

  interface Props {
    tabs: ReadonlyArray<TabItem>;
    active: string;
    onselect: (id: string) => void;
  }

  let { tabs, active, onselect }: Props = $props();
</script>

<div class="tabs" role="tablist">
  {#each tabs as t (t.id)}
    <button
      type="button"
      role="tab"
      class:on={t.id === active}
      aria-selected={t.id === active}
      tabindex={t.id === active ? 0 : -1}
      onclick={() => onselect(t.id)}>{t.label}</button
    >
  {/each}
</div>

<style>
  .tabs {
    display: flex;
    align-items: stretch;
    gap: 2px;
    width: 100%;
    box-sizing: border-box;
    padding: 2px;
    background: #101010;
    border: 1px solid #2a2a2a;
  }

  .tabs button {
    flex: 1;
    appearance: none;
    background: #1a1a1a;
    color: #8b939c;
    border: 1px solid transparent;
    border-radius: 2px;
    padding: 3px 6px;
    font-family: "Silkscreen", ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 8px;
    letter-spacing: 0.14em;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    cursor: pointer;
  }

  .tabs button:hover {
    color: #3ccb4e;
  }

  .tabs button.on {
    background: #161616;
    border-color: #3ccb4e;
    color: #3ccb4e;
  }
</style>
