<script lang="ts">
  import { onDestroy, untrack } from "svelte";
  import ResourceTable from "./ResourceTable.svelte";
  import type { ResourceKind } from "../bindings";
  import type { Row } from "../columns";
  import { app } from "../stores/app.svelte";
  import { detail, openDetail, targetKey } from "../stores/detail.svelte";
  import { live } from "../stores/live.svelte";

  let { kind }: { kind: ResourceKind } = $props();

  // Start the watches on mount and on Refresh. The view itself is switched
  // by remounting this component.
  $effect(() => {
    void app.refreshNonce;
    untrack(() => live.start(kind, app.selectedNamespaces));
  });

  // A namespace picked or dropped starts or stops just its own watch.
  $effect(() => {
    const namespaces = app.selectedNamespaces;
    untrack(() => live.setNamespaces(namespaces));
  });

  onDestroy(() => live.stop());

  const selectedKey = $derived(detail.target ? targetKey(detail.target) : null);

  function onselect(row: Row) {
    openDetail({ resource: kind.resource, namespace: row.namespace, name: row.name });
  }

  function oncount(shown: number, total: number) {
    app.count = `${shown} of ${total} items`;
  }
</script>

<ResourceTable
  {kind}
  store={live.store}
  metrics={live.metrics}
  now={live.now}
  search={app.search}
  {selectedKey}
  {onselect}
  {oncount}
/>
