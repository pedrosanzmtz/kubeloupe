<script lang="ts">
  import { onMount } from "svelte";
  import Header from "./lib/components/Header.svelte";
  import Sidebar from "./lib/components/Sidebar.svelte";
  import ErrorBanner from "./lib/components/ErrorBanner.svelte";
  import Footer from "./lib/components/Footer.svelte";
  import Toolbar from "./lib/components/Toolbar.svelte";
  import Overview from "./lib/components/Overview.svelte";
  import { app, loadContexts, loadNamespaces } from "./lib/stores/app.svelte";

  onMount(async () => {
    await loadContexts();
    await loadNamespaces();
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
      <p class="placeholder muted">
        The {app.view} table is being ported to Svelte and arrives in the next change.
      </p>
    {/if}
  </section>
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
  .placeholder {
    text-align: center;
    padding: 32px 0;
  }
</style>
