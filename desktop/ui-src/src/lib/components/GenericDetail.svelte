<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import ActionBar from "./ActionBar.svelte";
  import ConditionsTable from "./ConditionsTable.svelte";
  import DetailNotices from "./DetailNotices.svelte";
  import EventsTable from "./EventsTable.svelte";
  import KindSummary from "./KindSummary.svelte";
  import MetaSection from "./MetaSection.svelte";
  import RelatedGroups from "./RelatedGroups.svelte";
  import YamlEditor from "./YamlEditor.svelte";
  import { isWorkloadResource } from "../dock";
  import { closeDetail } from "../stores/detail.svelte";
  import { openDockTab } from "../stores/dock.svelte";
  import { registerTabs } from "../stores/keyboard.svelte";
  import type { Target } from "../stores/detail.svelte";
  import { LiveDetail } from "../stores/live-detail.svelte";
  import { LiveRelated } from "../stores/related.svelte";

  /** Detail panel for any kind without a dedicated one (pods have their own). */
  let { target }: { target: Target } = $props();

  // The panel is remounted for every object, so `target` is fixed here.
  // svelte-ignore state_referenced_locally
  const live = new LiveDetail(target);
  const info = $derived(live.info);
  // svelte-ignore state_referenced_locally
  const related = new LiveRelated(target);
  /** Kinds without related objects (a CRD, a Lease) show none of it. */
  const hasRelated = $derived.by(() => {
    const d = related.data;
    return !!d && (d.summary.length > 0 || d.facts.length > 0 || d.groups.length > 0);
  });
  // Reload the related objects when the live detail reports a change.
  $effect(() => {
    if (live.info) related.seen(live.info);
  });

  /** Workloads get a Logs button: their pods' logs, merged, in a dock tab. */
  const isWorkload = $derived(isWorkloadResource(target.resource));
  const workload = $derived({ kind: target.resource.kind, namespace: target.namespace, name: target.name });
  const tabs = $derived([
    { id: "overview", label: "Overview" },
    { id: "yaml", label: "YAML" },
  ]);
  let tab = $state("overview");

  onMount(() => {
    void live.start();
    void related.refresh();
  });
  onDestroy(() => {
    live.stop();
    related.stop();
  });
  // Ctrl/Cmd+1…9 picks a tab.
  onMount(() => registerTabs({ count: () => tabs.length, select: (i) => (tab = tabs[i].id) }));

  const title = $derived(target.namespace ? `${target.namespace}/${target.name}` : target.name);
</script>

<aside data-testid="detail-panel">
  <div class="header">
    <span class="title"><span class="muted">{target.resource.kind}</span> {title}</span>
    <button type="button" class="close" aria-label="Close details" onclick={closeDetail}>✕</button>
  </div>

  <ActionBar {target} disabled={live.deleted} />
  <DetailNotices {live} kind={target.resource.kind} />

  <div class="tabs" role="tablist">
    {#each tabs as t (t.id)}
      <button
        type="button"
        role="tab"
        aria-selected={tab === t.id}
        class:active={tab === t.id}
        onclick={() => (tab = t.id)}>{t.label}</button
      >
    {/each}
    {#if isWorkload}
      <button
        type="button"
        class="btn dock-logs"
        title="Opens the merged logs of its pods in the dock below"
        onclick={() => openDockTab({ type: "workloadLogs", workload })}>Logs</button
      >
    {/if}
  </div>

  {#if tab === "overview"}
    <div class="body">
      {#if live.loadError && !info}
        <p class="error-text">Could not load details: {live.loadError}</p>
      {:else if !info}
        <p class="muted">Loading…</p>
      {:else}
        {#if hasRelated}<KindSummary {related} />{/if}
        <MetaSection {info} />

        {#if info.secret_keys.length > 0}
          <h3>Data</h3>
          <p class="muted small">Values are hidden. Reveal them in the YAML tab.</p>
          <ul class="plain">
            {#each info.secret_keys as k (k.key)}
              <li class="mono">{k.key} <span class="muted">({k.bytes} bytes)</span></li>
            {/each}
          </ul>
        {/if}

        <RelatedGroups {related} />
        <ConditionsTable conditions={info.conditions} />
        <EventsTable events={info.events} />
      {/if}
    </div>
  {:else if tab === "yaml"}
    <YamlEditor {target} />
  {/if}
</aside>

<style>
  aside {
    width: 420px;
    flex-shrink: 0;
    background: var(--bg-raised);
    border-left: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow-y: auto;
  }
  aside > :global(*) {
    flex-shrink: 0;
  }
  .header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border);
    font-weight: 600;
    font-size: 13px;
    word-break: break-all;
  }
  .close {
    background: transparent;
    color: var(--text-muted);
    border: none;
    font-size: 14px;
    cursor: pointer;
    flex-shrink: 0;
  }
  .close:hover {
    color: var(--text);
  }
  .tabs {
    display: flex;
    gap: 4px;
    padding: 0 12px;
    border-bottom: 1px solid var(--border);
  }
  .tabs button[role="tab"] {
    background: transparent;
    border: none;
    border-bottom: 2px solid transparent;
    color: var(--text-muted);
    padding: 8px 10px;
    font-size: 13px;
    cursor: pointer;
  }
  .tabs button[role="tab"].active {
    color: var(--text);
    border-bottom-color: var(--accent);
  }
  .tabs .dock-logs {
    margin: 4px 0 4px auto;
    align-self: center;
  }
  .body {
    padding: 12px 16px;
  }
  .body h3 {
    margin: 14px 0 8px;
    font-size: 13px;
  }
  .mono {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 12px;
  }
  .small {
    font-size: 12px;
    margin: 0 0 6px;
  }
  ul.plain {
    list-style: none;
    margin: 0;
    padding: 0;
    font-size: 13px;
  }
  ul.plain li {
    padding: 2px 0;
  }
</style>
