<script lang="ts">
  import VirtualList from "./VirtualList.svelte";
  import { columnsFor } from "../columns";
  import type { Kind, Metrics, Row } from "../columns";
  import type { KeyedStore } from "../stores/keyed.svelte";
  import { visibleEntries } from "../view";
  import type { Sort } from "../view";

  interface Props {
    kind: Kind;
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
  const template = $derived(columns.map((c) => c.width).join(" "));
  let sort = $state.raw<Sort | null>(null);

  // Re-evaluated when the rows, the filter, the sort, or (only when sorting by
  // a metric / age column) the metrics or clock change.
  const visible = $derived(visibleEntries(store.entries, search, sort, columns, { metrics, now }));
  const getKey = $derived((i: number) => visible[i]?.key ?? i);

  $effect(() => {
    oncount?.(visible.length, store.entries.length);
  });

  function toggleSort(key: string) {
    if (sort?.key !== key) sort = { key, dir: "asc" };
    else if (sort.dir === "asc") sort = { key, dir: "desc" };
    else sort = null;
  }
</script>

<div class="table" role="table">
  <div class="head" role="row" style:grid-template-columns={template}>
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
          class:clickable={!!onselect}
          class:selected={entry.key === selectedKey}
          role="row"
          tabindex="-1"
          style:grid-template-columns={template}
          onclick={() => onselect?.(entry.row)}
        >
          {#each columns as col (col.key)}
            <div class="td {col.cls?.(entry.row) ?? ''}" role="cell" title={col.text(entry.row, { metrics, now })}>
              {col.text(entry.row, { metrics, now })}
            </div>
          {/each}
        </div>
      {/if}
    {/snippet}
  </VirtualList>

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
