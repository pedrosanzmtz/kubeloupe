<script lang="ts">
  import { app, switchNamespace } from "../stores/app.svelte";
</script>

<div class="toolbar">
  <select
    class="select"
    class:invisible={app.view !== "pods"}
    aria-label="Namespace"
    value={app.namespace}
    onchange={(e) => switchNamespace(e.currentTarget.value)}
  >
    <option value="">All namespaces</option>
    {#each app.namespaces as ns (ns.name)}
      <option value={ns.name}>{ns.name}</option>
    {/each}
  </select>
  <input
    class="input search"
    type="search"
    placeholder="Filter…"
    aria-label="Filter"
    bind:value={app.search}
  />
</div>

<style>
  .toolbar {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border);
  }
  .invisible {
    visibility: hidden;
  }
  .search {
    width: 220px;
  }
</style>
