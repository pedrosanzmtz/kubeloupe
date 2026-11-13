<script lang="ts">
  import { onDestroy, untrack } from "svelte";
  import ResourceTable from "./ResourceTable.svelte";
  import { podKey } from "../columns";
  import type { Kind, Row } from "../columns";
  import type { PodInfo } from "../bindings";
  import { app } from "../stores/app.svelte";
  import { detail, openDetail } from "../stores/detail.svelte";
  import { live } from "../stores/live.svelte";

  let { kind }: { kind: Kind } = $props();

  // (Re)start the watch when the namespace changes or Refresh is pressed.
  // The view itself is switched by remounting this component.
  $effect(() => {
    const ns = app.namespace;
    void app.refreshNonce;
    untrack(() => live.start(kind, ns || null));
  });

  onDestroy(() => live.stop());

  const selectedKey = $derived(kind === "pods" && detail.pod ? podKey(detail.pod) : null);

  function onselect(row: Row) {
    if (kind === "pods") openDetail(row as PodInfo);
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
  onselect={kind === "pods" ? onselect : undefined}
  {oncount}
/>
