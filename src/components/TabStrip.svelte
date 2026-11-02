<script lang="ts">
  /**
   * Reusable tab strip for the non-skin windows (#16): Winamp-ish dark
   * chrome that sits under a `TitleStrip`, one button per pane. Pure
   * props/callbacks — the caller owns which pane is mounted and any
   * persistence of the active tab.
   *
   * Implements the WAI-ARIA tabs pattern with automatic activation:
   * roving `tabindex` (only the active tab is tab-reachable) plus
   * ArrowLeft/ArrowRight (wrapping) and Home/End, which both select and
   * focus the target. Clicks also move focus explicitly — WKWebView
   * inherits Safari's quirk of not focusing buttons on click, which
   * would otherwise desync the roving tabindex from `active`.
   *
   * Id convention for the pane side: a tab `foo` has button id `tab-foo`
   * and controls `pane-foo` — the caller's pane carries
   * `id="pane-foo"`, `role="tabpanel"` and `aria-labelledby="tab-foo"`.
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

  // Roving focus needs to reach the button of the NEWLY active tab before
  // Svelte re-renders; all buttons exist at all times, so a live map is
  // enough (member-expression binding keeps it current; Svelte nulls the
  // entry on unmount, which focus() safely ignores).
  const buttons = new Map<string, HTMLElement | undefined>();

  function select(id: string): void {
    onselect(id);
    buttons.get(id)?.focus();
  }

  function onKeydown(e: KeyboardEvent, t: TabItem): void {
    const n = tabs.length;
    const i = tabs.findIndex((x) => x.id === t.id);
    if (i < 0 || n === 0) return;
    let next = -1;
    switch (e.key) {
      case "ArrowRight":
        next = (i + 1) % n;
        break;
      case "ArrowLeft":
        next = (i - 1 + n) % n;
        break;
      case "Home":
        next = 0;
        break;
      case "End":
        next = n - 1;
        break;
    }
    if (next < 0) return;
    e.preventDefault();
    const id = tabs[next]?.id;
    if (id != null && id !== t.id) select(id);
  }
</script>

<div class="tabs" role="tablist">
  {#each tabs as t (t.id)}
    <button
      type="button"
      role="tab"
      id="tab-{t.id}"
      aria-controls="pane-{t.id}"
      class:on={t.id === active}
      aria-selected={t.id === active}
      tabindex={t.id === active ? 0 : -1}
      bind:this={buttons[t.id]}
      onclick={() => select(t.id)}
      onkeydown={(e) => onKeydown(e, t)}>{t.label}</button
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

  .tabs button:focus-visible {
    border-color: #3ccb4e;
    outline: none;
  }

  .tabs button.on {
    background: #161616;
    border-color: #3ccb4e;
    color: #3ccb4e;
  }
</style>
