<script lang="ts">
  import { summaryLine } from "../related";
  import type { LiveRelated } from "../stores/related.svelte";

  /** The per-kind summary line and facts at the top of a detail panel, with
   *  the Refresh button for the related objects. */
  let { related }: { related: LiveRelated } = $props();
  const data = $derived(related.data);
  const line = $derived(summaryLine(data?.summary ?? []));
</script>

<div class="summary-row">
  <span class="summary" data-testid="kind-summary">{line}</span>
  <button
    type="button"
    class="btn"
    aria-label="Refresh related objects"
    disabled={related.loading}
    onclick={() => void related.refresh()}>{related.loading ? "Refreshing…" : "Refresh"}</button
  >
</div>

{#if data && data.facts.length > 0}
  <dl class="facts" data-testid="kind-facts">
    {#each data.facts as f, i (i)}
      <dt>{f.label}</dt>
      <dd>{f.value}</dd>
    {/each}
  </dl>
{/if}

<style>
  .summary-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin-bottom: 10px;
  }
  .summary {
    font-size: 13px;
    font-weight: 600;
  }
  .btn {
    flex-shrink: 0;
  }
  dl.facts {
    display: grid;
    grid-template-columns: 90px 1fr;
    gap: 4px 12px;
    margin: 0 0 12px;
    font-size: 13px;
  }
  dl.facts dt {
    color: var(--text-muted);
  }
  dl.facts dd {
    margin: 0;
    word-break: break-word;
  }
</style>
