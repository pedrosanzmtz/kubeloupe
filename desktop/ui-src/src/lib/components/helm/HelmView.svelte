<script lang="ts">
  import { onDestroy, onMount, untrack } from "svelte";
  import VirtualList from "../VirtualList.svelte";
  import { app } from "../../stores/app.svelte";
  import { checkHelm, helm, loadReleases, resetHelmView, selectRelease } from "../../stores/helm.svelte";
  import { ageOf, chartLabel, releaseKey, statusClass, visibleReleases } from "../../helm";

  /** The Helm releases table. Rows come from labels; charts fill in after. */

  const ROW_HEIGHT = 32;
  const template = "minmax(140px, 2fr) minmax(100px, 1fr) 80px minmax(140px, 1.5fr) 110px 120px 80px";
  const HEADERS = ["Name", "Namespace", "Revision", "Chart", "App version", "Status", "Updated"];

  let now = $state(Date.now());
  onMount(() => {
    void checkHelm();
    const t = setInterval(() => (now = Date.now()), 30_000);
    return () => clearInterval(t);
  });
  onDestroy(resetHelmView);

  // Load on mount, on Refresh and when the namespaces change.
  $effect(() => {
    void app.refreshNonce;
    const namespaces = app.selectedNamespaces;
    untrack(() => {
      now = Date.now();
      void loadReleases(namespaces);
    });
  });

  const visible = $derived(visibleReleases(helm.rows, helm.charts, app.search));
  const getKey = $derived((i: number) => (visible[i] ? releaseKey(visible[i]) : i));
  const selectedKey = $derived(helm.selected ? releaseKey(helm.selected) : null);

  $effect(() => {
    app.count = `${visible.length} of ${helm.rows.length} releases`;
  });
</script>

<div class="table" data-testid="helm-releases">
  {#if helm.error}
    <p class="error-text banner" role="alert">Could not list Helm releases: {helm.error}</p>
  {/if}
  {#each helm.warnings as w (w)}
    <p class="warning banner" data-testid="helm-warning">{w}</p>
  {/each}

  <div class="grid" role="grid" aria-label="Helm releases" aria-rowcount={visible.length + 1}>
    <div class="head" role="row" style:grid-template-columns={template}>
      {#each HEADERS as h (h)}
        <div class="th" role="columnheader">{h}</div>
      {/each}
    </div>
    <VirtualList count={visible.length} rowHeight={ROW_HEIGHT} {getKey}>
      {#snippet row(i)}
        {@const r = visible[i]}
        {#if r}
          {@const c = helm.charts.get(releaseKey(r))}
          {@const chart = c && c.revision === r.revision ? c : null}
          <!-- svelte-ignore a11y_click_events_have_key_events, a11y_interactive_supports_focus -->
          <div
            class="tr"
            role="row"
            data-testid="helm-row"
            class:selected={releaseKey(r) === selectedKey}
            style:grid-template-columns={template}
            onclick={() => selectRelease({ namespace: r.namespace, name: r.name })}
          >
            <div class="td" role="gridcell" title={r.name}>{r.name}</div>
            <div class="td" role="gridcell">{r.namespace}</div>
            <div class="td" role="gridcell">{r.revision}</div>
            <div class="td" role="gridcell" title={chart?.error ?? ""}>
              {#if chart?.error}<span class="muted">?</span>{:else if chart}{chartLabel(
                  chart.chart,
                  chart.chart_version,
                )}{:else}<span class="muted">…</span>{/if}
            </div>
            <div class="td" role="gridcell">{chart?.app_version ?? ""}</div>
            <div class="td {statusClass(r.status)}" role="gridcell">{r.status}</div>
            <div class="td" role="gridcell">{ageOf(r.updated, now)}</div>
          </div>
        {/if}
      {/snippet}
    </VirtualList>
  </div>

  {#if helm.loaded && visible.length === 0 && !helm.error}
    <p class="empty">{helm.rows.length ? "No releases match the filter." : "No Helm releases found."}</p>
  {:else if !helm.loaded}
    <p class="empty">Loading…</p>
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
  .banner {
    margin: 8px 0 0;
    font-size: 12px;
  }
  .warning {
    color: var(--amber);
  }
  .grid {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .head {
    display: grid;
    height: 28px;
    flex-shrink: 0;
    padding-right: 15px;
    border-bottom: 1px solid var(--border);
    align-items: center;
  }
  .th {
    padding: 0 10px;
    color: var(--text-muted);
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    white-space: nowrap;
  }
  .tr {
    display: grid;
    align-items: center;
    height: 100%;
    border-bottom: 1px solid var(--border);
    cursor: pointer;
  }
  .tr:hover,
  .tr.selected {
    background: var(--bg-hover);
  }
  .td {
    padding: 0 10px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13px;
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
