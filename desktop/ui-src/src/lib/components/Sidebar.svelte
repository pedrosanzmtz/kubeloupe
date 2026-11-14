<script lang="ts">
  import type { Category, ResourceKind } from "../bindings";
  import { app, canOpenDenied, deniedReason, kinds, setView } from "../stores/app.svelte";
  import { HELM_VIEW } from "../helm";

  const SECTIONS: { category: Category; title: string }[] = [
    { category: "workloads", title: "Workloads" },
    { category: "network", title: "Network" },
    { category: "config", title: "Config" },
    { category: "storage", title: "Storage" },
    { category: "cluster", title: "Cluster" },
  ];

  const byCategory = $derived.by(() => {
    const out = new Map<Category, ResourceKind[]>();
    for (const k of kinds.all) {
      const list = out.get(k.category) ?? [];
      list.push(k);
      out.set(k.category, list);
    }
    return out;
  });

  /** Custom resources (and anything unclassified) grouped by API group. */
  function byGroup(category: Category): [string, ResourceKind[]][] {
    const groups = new Map<string, ResourceKind[]>();
    for (const k of byCategory.get(category) ?? []) {
      const list = groups.get(k.resource.group) ?? [];
      list.push(k);
      groups.set(k.resource.group, list);
    }
    return [...groups.entries()].sort(([a], [b]) => a.localeCompare(b));
  }

  const custom = $derived(byGroup("custom"));
  const other = $derived(byGroup("other"));

  function open(kind: ResourceKind) {
    if (!kinds.denied.has(kind.id) || canOpenDenied(kind)) setView(kind.id);
  }
</script>

{#snippet item(kind: ResourceKind)}
  {@const denied = kinds.denied.has(kind.id)}
  <button
    type="button"
    class:active={app.view === kind.id}
    class:denied
    aria-disabled={denied && !canOpenDenied(kind)}
    title={denied ? deniedReason(kind) : kind.resource.kind}
    onclick={() => open(kind)}
  >
    {kind.label}
  </button>
{/snippet}

{#snippet groups(title: string, list: [string, ResourceKind[]][])}
  {#if list.length > 0}
    <h4>{title}</h4>
    {#each list as [group, items] (group)}
      <details open={items.some((k) => k.id === app.view)}>
        <summary title={group}>{group || "core"}</summary>
        {#each items as kind (kind.id)}
          {@render item(kind)}
        {/each}
      </details>
    {/each}
  {/if}
{/snippet}

<nav>
  <button type="button" class:active={app.view === "overview"} onclick={() => setView("overview")}>
    Overview
  </button>

  {#each SECTIONS as section (section.category)}
    {@const list = byCategory.get(section.category) ?? []}
    {#if list.length > 0}
      <h4>{section.title}</h4>
      {#each list as kind (kind.id)}
        {@render item(kind)}
      {/each}
    {/if}
  {/each}

  {#if kinds.loaded && kinds.all.length > 0}
    <h4>Helm</h4>
    <button type="button" class:active={app.view === HELM_VIEW} onclick={() => setView(HELM_VIEW)}>Releases</button>
  {/if}

  {@render groups("Custom Resources", custom)}
  {@render groups("Other", other)}

  {#if !kinds.loaded}
    <p class="muted note">Discovering…</p>
  {/if}
</nav>

<style>
  nav {
    width: 200px;
    flex-shrink: 0;
    background: var(--bg-raised);
    border-right: 1px solid var(--border);
    padding: 12px 8px;
    display: flex;
    flex-direction: column;
    gap: 2px;
    overflow-y: auto;
  }
  h4 {
    margin: 14px 12px 4px;
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-muted);
  }
  summary {
    padding: 6px 12px;
    font-size: 12px;
    color: var(--text-muted);
    cursor: pointer;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  details button {
    padding-left: 24px;
  }
  .note {
    margin: 12px;
    font-size: 12px;
  }
  button {
    text-align: left;
    background: transparent;
    color: var(--text-muted);
    border: none;
    border-radius: 6px;
    padding: 6px 12px;
    font-size: 13px;
    cursor: pointer;
  }
  button:hover {
    background: var(--bg-hover);
    color: var(--text);
  }
  button.active {
    background: var(--bg-hover);
    color: var(--text);
    font-weight: 600;
  }
  button.denied {
    opacity: 0.4;
    cursor: not-allowed;
  }
  button.denied:hover {
    background: transparent;
    color: var(--text-muted);
  }
  /* Denied across all namespaces only: opens, to pick a namespace there. */
  button.denied[aria-disabled="false"] {
    cursor: pointer;
  }
</style>
