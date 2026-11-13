<script lang="ts">
  import { onMount } from "svelte";
  import Header from "./lib/components/Header.svelte";
  import Sidebar from "./lib/components/Sidebar.svelte";
  import ErrorBanner from "./lib/components/ErrorBanner.svelte";
  import Footer from "./lib/components/Footer.svelte";
  import Toolbar from "./lib/components/Toolbar.svelte";
  import Overview from "./lib/components/Overview.svelte";
  import TableView from "./lib/components/TableView.svelte";
  import DetailPanel from "./lib/components/DetailPanel.svelte";
  import { app, loadContexts, loadNamespaces } from "./lib/stores/app.svelte";
  import { detail } from "./lib/stores/detail.svelte";
  import { live } from "./lib/stores/live.svelte";

  onMount(() => {
    // Subscribe before the first watch starts; events that still race ahead
    // of cmd_watch_start's reply are buffered by the live store.
    const unlisten = live.listen();
    (async () => {
      await loadContexts();
      await loadNamespaces();
    })();
    return () => {
      unlisten.then((un) => un());
    };
  });
</script>

<Header />
<div class="main">
  <Sidebar />
  <section class="content">
    {#if app.view !== "overview"}
      <Toolbar />
    {/if}
    <ErrorBanner />
    {#if app.view === "overview"}
      <Overview />
    {:else}
      {#key app.view}
        <TableView kind={app.view} />
      {/key}
    {/if}
  </section>
  {#if detail.pod && app.view === "pods"}
    {#key detail.nonce}
      <DetailPanel pod={detail.pod} />
    {/key}
  {/if}
</div>
<Footer />

<style>
  .main {
    display: flex;
    flex: 1;
    min-height: 0;
  }
  .content {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
</style>
