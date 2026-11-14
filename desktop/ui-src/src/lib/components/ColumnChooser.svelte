<script lang="ts">
  import { tick, untrack } from "svelte";
  import type { ResourceKind } from "../bindings";
  import { columnsFor } from "../columns";
  import { ALWAYS_SHOWN, toggleHidden } from "../settings";
  import { keyboard } from "../stores/keyboard.svelte";
  import { kindSettings, updateKind } from "../stores/settings.svelte";

  /** Show or hide the table's columns; remembered per kind. */
  let { kind }: { kind: ResourceKind } = $props();

  let open = $state(false);
  let root: HTMLDivElement | undefined = $state();
  let trigger: HTMLButtonElement | undefined = $state();

  const columns = $derived(columnsFor(kind));
  const hidden = $derived(kindSettings(kind.id).hidden);
  const hiddenCount = $derived(columns.filter((c) => hidden.includes(c.key)).length);

  // While open it handles its own keys (Esc), like a context menu.
  $effect(() => {
    if (!open) return;
    untrack(() => (keyboard.menus += 1));
    return () => {
      untrack(() => (keyboard.menus -= 1));
    };
  });

  async function show() {
    open = true;
    await tick();
    root?.querySelector<HTMLInputElement>(".popover input:not(:disabled)")?.focus();
  }

  function hide(focusTrigger = false) {
    open = false;
    if (focusTrigger) trigger?.focus();
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape" && open) {
      e.preventDefault();
      hide(true);
    }
  }

  function onOutside(e: PointerEvent) {
    if (open && root && !root.contains(e.target as Node)) hide();
  }
</script>

<svelte:window onpointerdown={onOutside} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="chooser" bind:this={root} onkeydown={onKey}>
  <button
    type="button"
    class="btn"
    aria-haspopup="dialog"
    aria-expanded={open}
    title="Show or hide columns"
    bind:this={trigger}
    onclick={() => (open ? hide() : show())}>Columns{hiddenCount ? ` (${hiddenCount} hidden)` : ""}</button
  >
  {#if open}
    <div class="popover" role="dialog" aria-label="Columns">
      {#each columns as col (col.key)}
        <label>
          <input
            type="checkbox"
            checked={!hidden.includes(col.key)}
            disabled={ALWAYS_SHOWN.includes(col.key)}
            onchange={() => updateKind(kind.id, { hidden: toggleHidden(hidden, col.key) })}
          />
          {col.label}
        </label>
      {/each}
      {#if hiddenCount}
        <button type="button" class="btn reset" onclick={() => updateKind(kind.id, { hidden: [] })}>Show all</button>
      {/if}
    </div>
  {/if}
</div>

<style>
  .chooser {
    position: relative;
  }
  .popover {
    position: absolute;
    top: calc(100% + 4px);
    right: 0;
    z-index: 50;
    min-width: 180px;
    max-height: 60vh;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 8px;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 6px 20px rgba(0, 0, 0, 0.35);
  }
  label {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 6px;
    font-size: 13px;
    border-radius: 4px;
    white-space: nowrap;
  }
  label:hover {
    background: var(--bg-hover);
  }
  .reset {
    margin-top: 6px;
  }
</style>
