<script lang="ts">
  import { onMount, untrack } from "svelte";
  import Header from "./lib/components/Header.svelte";
  import Sidebar from "./lib/components/Sidebar.svelte";
  import ErrorBanner from "./lib/components/ErrorBanner.svelte";
  import ActionDialog from "./lib/components/ActionDialog.svelte";
  import ActionNotice from "./lib/components/ActionNotice.svelte";
  import UnsavedDialog from "./lib/components/UnsavedDialog.svelte";
  import CreateDialog from "./lib/components/CreateDialog.svelte";
  import { createUi } from "./lib/stores/create.svelte";
  import Footer from "./lib/components/Footer.svelte";
  import Toolbar from "./lib/components/Toolbar.svelte";
  import Overview from "./lib/components/Overview.svelte";
  import TableView from "./lib/components/TableView.svelte";
  import DetailPanel from "./lib/components/DetailPanel.svelte";
  import GenericDetail from "./lib/components/GenericDetail.svelte";
  import PortForwards from "./lib/components/PortForwards.svelte";
  import { PORT_FORWARDS_VIEW } from "./lib/forwards";
  import Dock from "./lib/components/Dock.svelte";
  import { dock } from "./lib/stores/dock.svelte";
  import { isPod } from "./lib/columns";
  import {
    app,
    checkAccess,
    currentKind,
    initScope,
    loadContexts,
    loadKinds,
    loadNamespaces,
  } from "./lib/stores/app.svelte";
  import { detail } from "./lib/stores/detail.svelte";
  import { listenDrain } from "./lib/stores/actions.svelte";
  import { live } from "./lib/stores/live.svelte";
  import { handleKey, keyboard } from "./lib/stores/keyboard.svelte";
  import CommandPalette from "./lib/components/CommandPalette.svelte";
  import ShortcutHelp from "./lib/components/ShortcutHelp.svelte";

  onMount(() => {
    // Subscribe before the first watch starts; events that still race ahead
    // of cmd_watch_start's reply are buffered by the live store.
    const unlisten = live.listen();
    const unlistenDrain = listenDrain();
    (async () => {
      await loadContexts();
      await loadNamespaces();
      await initScope();
    })();
    return () => {
      unlisten.then((un) => un());
      unlistenDrain.then((un) => un());
    };
  });

  // Discover what the cluster serves on start, on Refresh and on a context
  // switch (all bump the nonce).
  $effect(() => {
    void app.refreshNonce;
    untrack(loadKinds);
  });

  // Access depends on the scope: re-check when the namespace changes.
  $effect(() => {
    void app.selectedNamespaces;
    untrack(checkAccess);
  });

  const kind = $derived(currentKind());
</script>

<!-- Capture phase: shortcuts see the state before a menu or dialog reacts. -->
<svelte:window onkeydowncapture={handleKey} />

<Header />
<div class="main" class:hidden={dock.maximized}>
  <Sidebar />
  <section class="content">
    {#if app.view !== "overview" && app.view !== PORT_FORWARDS_VIEW}
      <Toolbar />
    {/if}
    <ErrorBanner />
    <ActionNotice />
    {#if app.view === "overview"}
      <Overview />
    {:else if app.view === PORT_FORWARDS_VIEW}
      <PortForwards />
    {:else if kind}
      {#key kind.id}
        <TableView {kind} />
      {/key}
    {/if}
  </section>
  {#if detail.target && kind && detail.target.resource.kind === kind.resource.kind &&
    detail.target.resource.group === kind.resource.group}
    {#key detail.nonce}
      {#if isPod(kind)}
        <DetailPanel target={detail.target} />
      {:else}
        <GenericDetail target={detail.target} />
      {/if}
    {/key}
  {/if}
</div>
<Dock />
<Footer />
<ActionDialog />
{#if createUi.open}
  <CreateDialog />
{/if}
<UnsavedDialog />
{#if keyboard.palette}
  <CommandPalette />
{/if}
{#if keyboard.help}
  <ShortcutHelp />
{/if}

<style>
  .main {
    display: flex;
    flex: 1;
    min-height: 0;
  }
  /* A maximized dock covers the tables; they stay mounted. */
  .main.hidden {
    display: none;
  }
  .content {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
</style>
