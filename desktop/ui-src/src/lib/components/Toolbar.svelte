<script lang="ts">
  import { app, currentKind, switchNamespace } from "../stores/app.svelte";

  // Only namespaced kinds can be narrowed to a namespace.
  const namespaced = $derived(currentKind()?.resource.namespaced ?? false);
</script>

<div class="toolbar">
  {#if app.namespacesForbidden}
    <!-- Namespaces cannot be listed: offer the known ones, accept any. -->
    <input
      class="input namespace"
      class:invisible={!namespaced}
      list="known-namespaces"
      placeholder="All namespaces"
      aria-label="Namespace"
      title="You may not list namespaces: type one you have access to"
      value={app.namespace}
      onchange={(e) => switchNamespace(e.currentTarget.value)}
    />
    <datalist id="known-namespaces">
      {#each app.knownNamespaces as ns (ns)}
        <option value={ns}></option>
      {/each}
    </datalist>
  {:else}
    <select
      class="select"
      class:invisible={!namespaced}
      aria-label="Namespace"
      value={app.namespace}
      onchange={(e) => switchNamespace(e.currentTarget.value)}
    >
      <option value="">All namespaces</option>
      {#each app.namespaces as ns (ns.name)}
        <option value={ns.name}>{ns.name}</option>
      {/each}
    </select>
  {/if}
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
  .namespace {
    width: 200px;
  }
</style>
