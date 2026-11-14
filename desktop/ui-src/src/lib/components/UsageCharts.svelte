<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import UsageChart from "./UsageChart.svelte";
  import type { MetricsKind } from "../bindings";
  import { formatCpu, formatMemory } from "../format";
  import { UsageFeed } from "../stores/usage.svelte";
  import { clockTime, refLines } from "../usage";
  import type { ChartLine } from "../usage";

  /** CPU and memory charts of a pod or node (#47), from the usage history the
   *  backend keeps while metrics are on screen. Pods also show each
   *  container, nodes their allocatable. */
  let { what, namespace, name }: { what: MetricsKind; namespace: string | null; name: string } = $props();

  // The panel is remounted for every object, so the props are fixed here.
  // svelte-ignore state_referenced_locally
  const feed = new UsageFeed(what, namespace, name);
  onMount(() => feed.start());
  onDestroy(() => feed.stop());

  const data = $derived(feed.data);
  const lines = $derived.by((): ChartLine[] => {
    const h = data?.history;
    if (!h) return [];
    const total = { name: what === "pods" ? "pod" : "node", samples: h.samples };
    // One container is the pod itself; more get a line each.
    return h.containers.length > 1 ? [total, ...h.containers] : [total];
  });
</script>

<section class="usage" data-testid="usage-charts">
  <h3>Usage</h3>
  {#if !data}
    <p class="muted small">{feed.error ? `Could not load usage: ${feed.error}` : "Loading…"}</p>
  {:else if data.history.samples.length === 0}
    <p class="muted small" data-testid="usage-empty">
      No usage yet. metrics-server has no sample for this {what === "pods" ? "pod" : "node"}, or is not installed.
    </p>
  {:else}
    <p class="muted small" data-testid="usage-since">
      Collecting since {clockTime(data.history.since)}. History starts when a view with metrics opens and keeps 30
      minutes.
    </p>
    <UsageChart title="CPU" resource="cpu" {lines} refs={refLines(data.reference, "cpu")} format={formatCpu} />
    <UsageChart
      title="Memory"
      resource="memory"
      {lines}
      refs={refLines(data.reference, "memory")}
      format={formatMemory}
    />
  {/if}
</section>

<style>
  h3 {
    margin: 14px 0 8px;
    font-size: 13px;
  }
  .small {
    font-size: 12px;
    margin: 0 0 8px;
  }
</style>
