<script lang="ts">
  import { onMount } from "svelte";
  import VirtualList from "./VirtualList.svelte";
  import ContextMenu from "./ContextMenu.svelte";
  import type { MenuItem } from "./ContextMenu.svelte";
  import { replicasFromRow } from "../actions";
  import { rangeKeys } from "../selection";
  import { requestAction } from "../stores/actions.svelte";
  import { dockMenuFor } from "../dock";
  import { openDockTab } from "../stores/dock.svelte";
  import { registerTable } from "../stores/keyboard.svelte";
  import type { RowMove } from "../stores/keyboard.svelte";
  import type { ResourceKind } from "../bindings";
  import { columnsFor } from "../columns";
  import type { Metrics, Row } from "../columns";
  import type { KeyedStore } from "../stores/keyed.svelte";
  import { visibleEntries } from "../view";
  import type { Sort } from "../view";
  import { sortOf, withSort } from "../settings";
  import { kindSettings, updateSettings } from "../stores/settings.svelte";

  interface Props {
    kind: ResourceKind;
    store: KeyedStore<Row>;
    metrics: Metrics;
    now: number;
    search?: string;
    /** Key of the highlighted row, if any. */
    selectedKey?: string | null;
    onselect?: (row: Row) => void;
    /** Called with (shown, total) whenever either changes. */
    oncount?: (shown: number, total: number) => void;
    scrollEl?: HTMLDivElement | null;
  }

  let {
    kind,
    store,
    metrics,
    now,
    search = "",
    selectedKey = null,
    onselect,
    oncount,
    scrollEl = $bindable(null),
  }: Props = $props();

  const ROW_HEIGHT = 28;

  // The columns the user has not hidden (column chooser in the toolbar),
  // and the sort, both remembered per kind.
  const hidden = $derived(kindSettings(kind.id).hidden);
  const columns = $derived(columnsFor(kind).filter((c) => !hidden.includes(c.key)));
  const template = $derived(["32px", ...columns.map((c) => c.width)].join(" "));
  // svelte-ignore state_referenced_locally
  let sort = $state.raw<Sort | null>(sortOf(kindSettings(kind.id)));

  // Re-evaluated when the rows, the filter, the sort, or (only when sorting by
  // a metric / age column) the metrics or clock change.
  const visible = $derived(visibleEntries(store.entries, search, sort, columns, { metrics, now }));
  const getKey = $derived((i: number) => visible[i]?.key ?? i);

  $effect(() => {
    oncount?.(visible.length, store.entries.length);
  });

  // Multi-select. Only rows that are on screen can stay selected, so what a
  // bulk action names is always what the user can see.
  let selected = $state.raw(new Set<string>());
  let anchor = $state<string | null>(null);
  const visibleKeys = $derived(visible.map((e) => e.key));
  const allSelected = $derived(visible.length > 0 && visible.every((e) => selected.has(e.key)));
  const someSelected = $derived(selected.size > 0 && !allSelected);
  const deleteAction = $derived(kind.actions.find((a) => a.id === "delete"));

  $effect(() => {
    const keys = new Set(visibleKeys);
    if ([...selected].some((k) => !keys.has(k))) {
      selected = new Set([...selected].filter((k) => keys.has(k)));
    }
  });

  function toggleRow(key: string, shift: boolean) {
    const next = new Set(selected);
    if (shift && anchor !== null) {
      const on = !selected.has(key);
      for (const k of rangeKeys(visibleKeys, anchor, key)) {
        if (on) next.add(k);
        else next.delete(k);
      }
    } else if (next.has(key)) next.delete(key);
    else next.add(key);
    selected = next;
    anchor = key;
  }

  function toggleAll() {
    selected = allSelected ? new Set() : new Set(visibleKeys);
    anchor = null;
  }

  const targetsOf = (keys: Set<string>) =>
    visible.filter((e) => keys.has(e.key)).map((e) => ({ namespace: e.row.namespace, name: e.row.name }));

  function deleteSelected() {
    if (deleteAction) requestAction(kind, targetsOf(selected), deleteAction);
  }

  // Right-click menu, built from the same registry as the detail panels.
  let menu = $state.raw<{ x: number; y: number; title: string; items: MenuItem[] } | null>(null);

  function openMenu(e: MouseEvent, row: Row) {
    e.preventDefault();
    focusedKey = row.key;
    openMenuAt(e.clientX, e.clientY, row);
  }

  function openMenuAt(x: number, y: number, row: Row) {
    let items: MenuItem[];
    let title: string;
    if (selected.has(row.key) && selected.size > 1) {
      title = `${selected.size} selected`;
      items = deleteAction
        ? [{ label: `Delete ${selected.size} selected…`, danger: true, run: deleteSelected }]
        : [];
    } else {
      title = row.namespace ? `${row.namespace}/${row.name}` : row.name;
      const keys = kind.columns.map((c) => c.key);
      // Logs and Shell open dock tabs; the rest come from the action registry.
      items = [
        ...dockMenuFor(kind.resource, row.namespace, row.name).map((d) => ({
          label: d.label,
          run: () => openDockTab(d.spec),
        })),
        ...kind.actions.map((a) => ({
          label: a.label,
          danger: a.destructive,
          run: () =>
            requestAction(kind, [{ namespace: row.namespace, name: row.name }], a, replicasFromRow(row, keys)),
        })),
      ];
    }
    menu = items.length > 0 ? { x, y, title, items } : null;
  }

  // Keyboard focus: one row, by key, so it stays on the same object when
  // watch batches reorder the table. Separate from the checkboxes.
  let focusedKey = $state<string | null>(null);
  let lastFocusedIndex = -1;
  const focusedIndex = $derived(focusedKey === null ? -1 : visibleKeys.indexOf(focusedKey));
  let gridEl = $state<HTMLDivElement | null>(null);
  let list: VirtualList | undefined = $state();

  // The focused row went away (deleted, filtered out): focus its neighbour.
  $effect(() => {
    if (focusedIndex >= 0) lastFocusedIndex = focusedIndex;
    else if (focusedKey !== null) {
      focusedKey = visibleKeys.length > 0 ? visibleKeys[Math.min(lastFocusedIndex, visibleKeys.length - 1)] : null;
    }
  });

  function focusRow(index: number) {
    const key = visibleKeys[index];
    if (key === undefined) return;
    focusedKey = key;
    list?.scrollToIndex(index);
    gridEl?.focus({ preventScroll: true });
  }

  function move(to: RowMove) {
    const last = visible.length - 1;
    if (last < 0) return;
    // Start from the row in the detail panel when nothing has focus yet.
    const from = focusedIndex >= 0 ? focusedIndex : selectedKey ? visibleKeys.indexOf(selectedKey) : -1;
    const next = {
      next: from < 0 ? 0 : Math.min(from + 1, last),
      prev: from < 0 ? 0 : Math.max(from - 1, 0),
      first: 0,
      last,
    }[to];
    focusRow(next);
  }

  const focusedEntry = () => (focusedIndex >= 0 ? visible[focusedIndex] : undefined);

  function menuForFocused() {
    const entry = focusedEntry();
    if (!entry) return;
    list?.scrollToIndex(focusedIndex);
    const rect = document.getElementById(rowId(focusedIndex))?.getBoundingClientRect();
    openMenuAt(rect ? rect.left + 40 : 100, rect ? rect.bottom : 100, entry.row);
  }

  function deleteRows() {
    if (!deleteAction) return;
    if (selected.size > 0) return deleteSelected();
    const entry = focusedEntry();
    if (entry) requestAction(kind, [{ namespace: entry.row.namespace, name: entry.row.name }], deleteAction);
  }

  onMount(() =>
    registerTable({
      get kind() {
        return kind;
      },
      move,
      openFocused: () => {
        const entry = focusedEntry();
        if (entry) onselect?.(entry.row);
      },
      toggleFocused: () => {
        if (focusedKey !== null && focusedIndex >= 0) toggleRow(focusedKey, false);
      },
      menuForFocused,
      deleteRows,
      rows: () => store.entries.map((e) => e.row),
      actionTargets: () => {
        if (selected.size > 0) return { targets: targetsOf(selected), row: null };
        const entry = focusedEntry() ?? visible.find((e) => e.key === selectedKey);
        return entry
          ? { targets: [{ namespace: entry.row.namespace, name: entry.row.name }], row: entry.row }
          : { targets: [], row: null };
      },
      focus: () => gridEl?.focus({ preventScroll: true }),
    }),
  );

  /** DOM id of row `i`, for `aria-activedescendant`. */
  const rowId = (i: number) => `resource-row-${i}`;

  function toggleSort(key: string) {
    if (sort?.key !== key) sort = { key, dir: "asc" };
    else if (sort.dir === "asc") sort = { key, dir: "desc" };
    else sort = null;
    updateSettings({ kinds: { [kind.id]: withSort(kindSettings(kind.id), sort) } });
  }
</script>

<div class="table" data-testid="resource-table">
  {#if selected.size > 0}
    <div class="bulk" role="toolbar" aria-label="Selection">
      <span>{selected.size} selected</span>
      {#if deleteAction}
        <button type="button" class="btn danger" onclick={deleteSelected}>Delete {selected.size}…</button>
      {/if}
      <button type="button" class="btn" onclick={() => (selected = new Set())}>Clear</button>
    </div>
  {/if}
  <!-- ARIA grid: the grid keeps DOM focus and points at the focused row. -->
  <div
    class="grid"
    role="grid"
    tabindex="0"
    aria-label={kind.label}
    aria-rowcount={visible.length + 1}
    aria-colcount={columns.length + 1}
    aria-multiselectable="true"
    aria-activedescendant={focusedIndex >= 0 ? rowId(focusedIndex) : undefined}
    bind:this={gridEl}
  >
    <div class="head" role="row" aria-rowindex={1} style:grid-template-columns={template}>
      <div class="th check" role="columnheader">
        <input
          type="checkbox"
          aria-label="Select all visible rows"
          checked={allSelected}
          indeterminate={someSelected}
          onchange={toggleAll}
        />
      </div>
      {#each columns as col (col.key)}
        <button
          type="button"
          class="th"
          role="columnheader"
          aria-sort={sort?.key === col.key ? (sort.dir === "asc" ? "ascending" : "descending") : "none"}
          onclick={() => toggleSort(col.key)}
        >
          {col.label}
          {#if sort?.key === col.key}<span class="arrow">{sort.dir === "asc" ? "▲" : "▼"}</span>{/if}
        </button>
      {/each}
    </div>

    <VirtualList bind:this={list} count={visible.length} rowHeight={ROW_HEIGHT} {getKey} bind:scrollEl>
      {#snippet row(i)}
        {@const entry = visible[i]}
        {#if entry}
          <!-- Rows take no focus: the grid does, and points at the focused row
               (aria-activedescendant); keys are handled in stores/keyboard.svelte.ts. -->
          <!-- svelte-ignore a11y_click_events_have_key_events, a11y_interactive_supports_focus -->
          <div
            class="tr"
            id={rowId(i)}
            data-testid="resource-row"
            class:clickable={!!onselect}
            class:selected={entry.key === selectedKey}
            class:checked={selected.has(entry.key)}
            class:focused={i === focusedIndex}
            role="row"
            aria-rowindex={i + 2}
            aria-selected={selected.has(entry.key)}
            style:grid-template-columns={template}
            onclick={() => {
              focusedKey = entry.key;
              onselect?.(entry.row);
            }}
            oncontextmenu={(e) => openMenu(e, entry.row)}
          >
            <div class="td check" role="gridcell">
              <input
                type="checkbox"
                aria-label="Select {entry.row.name}"
                checked={selected.has(entry.key)}
                onclick={(e) => {
                  e.stopPropagation();
                  toggleRow(entry.key, e.shiftKey);
                }}
                onchange={() => {}}
              />
            </div>
            {#each columns as col (col.key)}
              <div class="td {col.cls?.(entry.row) ?? ''}" role="gridcell" title={col.text(entry.row, { metrics, now })}>
                {col.text(entry.row, { metrics, now })}
              </div>
            {/each}
          </div>
        {/if}
      {/snippet}
    </VirtualList>
  </div>

  {#if menu}
    <ContextMenu {...menu} onclose={() => (menu = null)} />
  {/if}

  {#if visible.length === 0}
    <p class="empty">No items found.</p>
  {/if}
</div>

<style>
  .table {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    padding: 0 16px 16px;
    position: relative;
  }
  .grid {
    flex: 1;
    min-height: 0;
    min-width: 0;
    /* Columns wider than the table are cut off here, like the rows in the
       scroller. Without the clip the header labels overflow into the detail
       panel and, being text, paint over its background (#57). */
    overflow: hidden;
    display: flex;
    flex-direction: column;
    outline: none;
  }
  .grid:focus-visible .tr.focused {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  .tr.focused {
    box-shadow: inset 3px 0 0 var(--accent);
  }
  .head {
    display: grid;
    height: 28px;
    flex-shrink: 0;
    /* Match the scroller's reserved scrollbar gutter. */
    padding-right: 15px;
    border-bottom: 1px solid var(--border);
    background: var(--bg);
  }
  .th {
    text-align: left;
    padding: 0 10px;
    background: transparent;
    border: none;
    color: var(--text-muted);
    font: inherit;
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    cursor: pointer;
    white-space: nowrap;
  }
  .th:hover {
    color: var(--text);
  }
  .arrow {
    font-size: 9px;
    margin-left: 4px;
  }
  .tr {
    display: grid;
    align-items: center;
    height: 100%;
    border-bottom: 1px solid var(--border);
  }
  .tr:hover {
    background: var(--bg-hover);
  }
  .tr.clickable {
    cursor: pointer;
  }
  .tr.selected {
    background: var(--bg-hover);
  }
  .tr.checked {
    background: rgba(79, 156, 247, 0.12);
  }
  .check {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 0;
  }
  .bulk {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 0;
    font-size: 13px;
  }
  .btn.danger {
    border-color: var(--red);
    color: var(--error-text);
  }
  .td {
    padding: 0 10px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .empty {
    position: absolute;
    top: 56px;
    left: 0;
    right: 0;
    margin: 0;
    text-align: center;
    color: var(--text-muted);
  }
</style>
