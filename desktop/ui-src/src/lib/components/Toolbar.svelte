<script lang="ts">
  import { app, currentKind } from "../stores/app.svelte";
  import NamespacePicker from "./NamespacePicker.svelte";
  import ColumnChooser from "./ColumnChooser.svelte";
  import { openCreate } from "../stores/create.svelte";

  const kind = $derived(currentKind());
  // Only namespaced kinds can be narrowed to a namespace.
  const namespaced = $derived(kind?.resource.namespaced ?? false);
</script>

<div class="toolbar">
  <div class="namespaces" class:invisible={!namespaced}>
    <NamespacePicker />
  </div>
  <input
    id="table-filter"
    class="input search"
    type="search"
    placeholder="Filter…"
    aria-label="Filter"
    bind:value={app.search}
  />
  {#if kind}
    <ColumnChooser {kind} />
  {/if}
  <button type="button" class="btn primary create" title="Create resources from YAML" onclick={openCreate}
    >+ Create</button
  >
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
  .namespaces {
    min-width: 0;
  }
  .create {
    margin-left: auto;
  }
</style>
