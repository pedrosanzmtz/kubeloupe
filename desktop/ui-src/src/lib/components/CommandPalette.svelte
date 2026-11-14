<script lang="ts">
  import { onMount } from "svelte";
  import { rankItems } from "../palette";
  import type { Ranked } from "../palette";
  import { activeTable, closePalette } from "../stores/keyboard.svelte";
  import { paletteItems, recentIds, rememberPick } from "../stores/palette.svelte";

  /** Ctrl/Cmd+K: fuzzy search over kinds, namespaces, contexts, objects and actions. */

  // Built once per opening: the palette is remounted every time.
  const items = paletteItems();
  let query = $state("");
  let active = $state(0);
  let input = $state<HTMLInputElement | null>(null);
  let listEl = $state<HTMLUListElement | null>(null);

  const results: Ranked[] = $derived(rankItems(items, query, recentIds()));

  $effect(() => {
    void query;
    active = 0;
  });

  // Keep the active option in view.
  $effect(() => {
    listEl?.querySelector(`#palette-option-${active}`)?.scrollIntoView({ block: "nearest" });
  });

  onMount(() => {
    const before = document.activeElement as HTMLElement | null;
    input?.focus();
    return () => {
      // Back to where the user was, unless what ran (a dialog) took focus.
      setTimeout(() => {
        if (document.activeElement && document.activeElement !== document.body) return;
        if (before?.isConnected && before !== document.body) before.focus({ preventScroll: true });
        else activeTable()?.focus();
      });
    };
  });

  function run(r: Ranked | undefined) {
    if (!r) return;
    rememberPick(r.item.id);
    closePalette();
    r.item.run();
  }

  function onkeydown(e: KeyboardEvent) {
    const n = results.length;
    switch (e.key) {
      case "Escape":
        closePalette();
        break;
      case "ArrowDown":
        if (n) active = (active + 1) % n;
        break;
      case "ArrowUp":
        if (n) active = (active - 1 + n) % n;
        break;
      case "PageDown":
        if (n) active = Math.min(active + 8, n - 1);
        break;
      case "PageUp":
        if (n) active = Math.max(active - 8, 0);
        break;
      case "Enter":
        run(results[active]);
        break;
      default:
        return;
    }
    e.preventDefault();
    e.stopPropagation();
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={(e) => e.target === e.currentTarget && closePalette()}>
  <div class="palette" role="dialog" aria-modal="true" aria-label="Command palette" data-testid="command-palette">
    <input
      bind:this={input}
      bind:value={query}
      class="input"
      type="text"
      role="combobox"
      aria-expanded="true"
      aria-controls="palette-list"
      aria-autocomplete="list"
      aria-activedescendant={results.length ? `palette-option-${active}` : undefined}
      aria-label="Search kinds, objects, namespaces, contexts and actions"
      placeholder="Go to…   > actions   ns namespaces   ctx contexts"
      spellcheck="false"
      autocomplete="off"
      {onkeydown}
    />
    <ul id="palette-list" role="listbox" aria-label="Results" bind:this={listEl}>
      {#each results as r, i (r.item.id)}
        {#if i === 0 || results[i - 1].group !== r.group}
          <li class="group" role="presentation">{r.group}</li>
        {/if}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <li
          id="palette-option-{i}"
          role="option"
          aria-selected={i === active}
          class:active={i === active}
          onmousemove={() => (active = i)}
          onclick={() => run(r)}
        >
          <span class="label">{r.item.label}</span>
          {#if r.item.detail}<span class="detail">{r.item.detail}</span>{/if}
          {#if r.group === "Recent"}<span class="tag">{r.item.group}</span>{/if}
        </li>
      {:else}
        <li class="none" role="presentation">No matches</li>
      {/each}
    </ul>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.4);
    z-index: 120;
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding-top: 12vh;
  }
  .palette {
    width: min(560px, calc(100vw - 32px));
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 12px 36px rgba(0, 0, 0, 0.5);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .input {
    margin: 10px;
    font-size: 14px;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0 6px 6px;
    max-height: 50vh;
    overflow-y: auto;
  }
  li[role="option"] {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 6px 10px;
    border-radius: 4px;
    font-size: 13px;
    cursor: pointer;
  }
  li.active {
    background: var(--bg-hover);
  }
  .group {
    padding: 8px 10px 2px;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-muted);
  }
  .label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .detail,
  .tag {
    color: var(--text-muted);
    font-size: 12px;
    white-space: nowrap;
  }
  .tag {
    margin-left: auto;
  }
  .none {
    padding: 10px;
    color: var(--text-muted);
    font-size: 13px;
  }
</style>
