<script lang="ts">
  import { onMount } from "svelte";
  import LogViewer from "./LogViewer.svelte";
  import Terminal from "./Terminal.svelte";
  import { commands } from "../bindings";
  import type { LogContainer } from "../bindings";
  import type { DockTab } from "../dock";

  /** One dock tab. It stays mounted while other tabs are shown, so the log
   *  buffer and the terminal scrollback survive switching tabs. */
  let { tab, active }: { tab: DockTab; active: boolean } = $props();

  // A pod tab looks up the pod's containers first: the log picker lists them
  // and the shell opens in the first regular one.
  let containers = $state<LogContainer[]>([]);
  let shell = $state<string | null>(null);
  let ready = $state(false);

  onMount(async () => {
    if (tab.type === "localShell" || tab.type === "debug" || tab.type === "nodeShell") {
      ready = true;
      return;
    }
    if (tab.type === "podLogs" && tab.containers) containers = tab.containers;
    if (
      tab.type === "workloadLogs" ||
      (tab.type === "terminal" && tab.container !== undefined) ||
      (tab.type === "podLogs" && tab.containers)
    ) {
      shell = tab.type === "terminal" ? (tab.container ?? null) : null;
      ready = true;
      return;
    }
    try {
      const d = await commands.cmdGetResourceDetail(tab.pod.resource, tab.pod.namespace, tab.pod.name);
      containers = d.pod.log_containers;
      shell = d.pod.containers.find((c) => c.kind === "regular")?.name ?? null;
    } catch {
      // Let the stream or the exec report the problem (pod gone, forbidden).
    }
    ready = true;
  });
</script>

<div class="pane" class:hidden={!active} data-testid="dock-pane" data-tab={tab.id}>
  {#if tab.type === "podLogs"}
    <LogViewer pod={tab.pod} {containers} {ready} hidden={!active} />
  {:else if tab.type === "workloadLogs"}
    <LogViewer workload={tab.workload} hidden={!active} />
  {:else if tab.type === "localShell"}
    <Terminal target={{ kind: "local", namespace: tab.namespace }} {active} />
  {:else if tab.type === "debug"}
    <Terminal target={{ kind: "debug", pod: tab.pod, target: tab.target, image: tab.image }} {active} />
  {:else if tab.type === "nodeShell"}
    <Terminal target={{ kind: "nodeShell", node: tab.node, namespace: tab.namespace }} {active} />
  {:else if ready}
    <Terminal target={{ kind: "pod", pod: tab.pod, container: shell }} {active} />
  {/if}
</div>

<style>
  .pane {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  /* Hidden, not unmounted: the stream and its buffer keep running. */
  .pane.hidden {
    display: none;
  }
</style>
