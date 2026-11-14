<script lang="ts">
  import DockPane from "./DockPane.svelte";
  import { clampHeight } from "../dock";
  import { activateDockTab, closeDockTab, dock, toggleDock, toggleMaximized } from "../stores/dock.svelte";
  import { setDockHeight } from "../stores/settings.svelte";

  /** Room kept for the header and the tables above a resized dock. */
  const KEEP_ABOVE = 160;
  const SHORTCUT = "Ctrl+\u0060";

  let drag: { y: number; height: number } | null = null;

  function startResize(e: PointerEvent) {
    if (e.button !== 0) return;
    e.preventDefault();
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    drag = { y: e.clientY, height: dock.height };
  }

  function onResize(e: PointerEvent) {
    if (!drag) return;
    dock.height = clampHeight(drag.height + drag.y - e.clientY, window.innerHeight - KEEP_ABOVE);
  }

  function endResize() {
    if (drag && drag.height !== dock.height) setDockHeight(dock.height);
    drag = null;
  }

</script>

{#if dock.tabs.length > 0}
  <section
    class="dock"
    class:maximized={dock.maximized}
    class:collapsed={!dock.expanded}
    data-testid="dock"
    aria-label="Dock"
    style:height={dock.expanded && !dock.maximized ? `${dock.height}px` : null}
  >
    {#if dock.expanded && !dock.maximized}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="resizer"
        data-testid="dock-resizer"
        title="Drag to resize"
        onpointerdown={startResize}
        onpointermove={onResize}
        onpointerup={endResize}
        onpointercancel={endResize}
      ></div>
    {/if}
    <div class="strip">
      <div class="tabs" role="tablist">
        {#each dock.tabs as t (t.id)}
          <div class="tab" class:active={t.id === dock.active} role="presentation">
            <button
              type="button"
              role="tab"
              class="label"
              aria-selected={t.id === dock.active}
              title="{t.title} (double-click to maximize)"
              onclick={() => activateDockTab(t.id)}
              ondblclick={() => {
                activateDockTab(t.id);
                toggleMaximized();
              }}>{t.title}</button
            >
            <button type="button" class="close" aria-label="Close {t.title}" onclick={() => closeDockTab(t.id)}
              >✕</button
            >
          </div>
        {/each}
      </div>
      <button
        type="button"
        class="ctl"
        aria-label={dock.maximized ? "Restore dock" : "Maximize dock"}
        title={dock.maximized ? "Restore" : "Maximize"}
        onclick={toggleMaximized}>{dock.maximized ? "▭" : "□"}</button
      >
      <button
        type="button"
        class="ctl"
        aria-label={dock.expanded ? "Collapse dock" : "Expand dock"}
        title={dock.expanded ? `Collapse (${SHORTCUT})` : `Expand (${SHORTCUT})`}
        onclick={toggleDock}>{dock.expanded ? "▾" : "▴"}</button
      >
    </div>
    <div class="body">
      {#each dock.tabs as t (t.id)}
        <DockPane tab={t} active={dock.expanded && t.id === dock.active} />
      {/each}
    </div>
  </section>
{/if}

<style>
  .dock {
    position: relative;
    display: flex;
    flex-direction: column;
    flex-shrink: 0;
    min-height: 0;
    background: var(--bg-raised);
    border-top: 1px solid var(--border);
  }
  .dock.maximized {
    flex: 1;
  }
  .resizer {
    position: absolute;
    top: -3px;
    left: 0;
    right: 0;
    height: 6px;
    cursor: ns-resize;
    z-index: 5;
  }
  .resizer:hover {
    background: var(--accent);
    opacity: 0.4;
  }
  .strip {
    display: flex;
    align-items: stretch;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }
  .collapsed .strip {
    border-bottom: none;
  }
  .tabs {
    display: flex;
    flex: 1;
    min-width: 0;
    overflow-x: auto;
  }
  .tab {
    display: flex;
    align-items: center;
    border-right: 1px solid var(--border);
    border-bottom: 2px solid transparent;
    flex-shrink: 0;
  }
  .tab.active {
    border-bottom-color: var(--accent);
    background: var(--bg-hover);
  }
  .label {
    background: transparent;
    border: none;
    color: var(--text-muted);
    padding: 6px 4px 6px 12px;
    font-size: 12px;
    cursor: pointer;
    max-width: 260px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tab.active .label {
    color: var(--text);
  }
  .close,
  .ctl {
    background: transparent;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    font-size: 12px;
    padding: 4px 8px;
  }
  .close:hover,
  .ctl:hover {
    color: var(--text);
  }
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  /* Collapsed: only the strip shows, the tabs keep running underneath. */
  .collapsed .body {
    display: none;
  }
</style>
