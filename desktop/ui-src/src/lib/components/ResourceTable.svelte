<script lang="ts">
  import VirtualList from "./VirtualList.svelte";
  import ContextMenu from "./ContextMenu.svelte";
  import type { MenuItem } from "./ContextMenu.svelte";
  import { replicasFromRow } from "../actions";
  import { rangeKeys } from "../selection";
  import { requestAction } from "../stores/actions.svelte";
  import type { ResourceKind } from "../bindings";
  import { columnsFor } from "../columns";
  import type { Metrics, Row } from "../columns";
  import type { KeyedStore } from "../stores/keyed.svelte";
  import { visibleEntries } from "../view";
  import type { Sort } from "../view";

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

  const columns = $derived(columnsFor(kind));
  const template = $derived(["32px", ...columns.map((c) => c.width)].join(" "));
  let sort = $state.raw<Sort | null>(null);

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
      items = kind.actions.map((a) => ({
        label: a.label,
        danger: a.destructive,
        run: () =>
          requestAction(kind, [{ namespace: row.namespace, name: row.name }], a, replicasFromRow(row, keys)),
      }));
    }
    menu = items.length > 0 ? { x: e.clientX, y: e.clientY, title, items } : null;
  }

  function toggleSort(key: string) {
    if (sort?.key !== key) sort = { key, dir: "asc" };
    else if (sort.dir === "asc") sort = { key, dir: "desc" };
    else sort = null;
  }
</script>

<div class="table" role="table" data-testid="resource-table">
  {#if selected.size > 0}
    <div class="bulk" role="toolbar" aria-label="Selection">
      <span>{selected.size} selected</span>
      {#if deleteAction}
        <button type="button" class="btn danger" onclick={deleteSelected}>Delete {selected.size}…</button>
      {/if}
      <button type="button" class="btn" onclick={() => (selected = new Set())}>Clear</button>
    </div>
  {/if}
  <div class="head" role="row" style:grid-template-columns={template}>
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

  <VirtualList count={visible.length} rowHeight={ROW_HEIGHT} {getKey} bind:scrollEl>
    {#snippet row(i)}
      {@const entry = visible[i]}
      {#if entry}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          class="tr"
          data-testid="resource-row"
          class:clickable={!!onselect}
          class:selected={entry.key === selectedKey}
          class:checked={selected.has(entry.key)}
          role="row"
          tabindex="-1"
          style:grid-template-columns={template}
          onclick={() => onselect?.(entry.row)}
          oncontextmenu={(e) => openMenu(e, entry.row)}
        >
          <div class="td check" role="cell">
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
            <div class="td {col.cls?.(entry.row) ?? ''}" role="cell" title={col.text(entry.row, { metrics, now })}>
              {col.text(entry.row, { metrics, now })}
            </div>
          {/each}
        </div>
      {/if}
    {/snippet}
  </VirtualList>

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
    color: #f5a3a5;
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
