<script lang="ts">
  import { onMount } from "svelte";
  import LogViewer from "./LogViewer.svelte";
  import PortForward from "./PortForward.svelte";
  import Terminal from "./Terminal.svelte";
  import YamlEditor from "./YamlEditor.svelte";
  import { commands } from "../bindings";
  import type { PodDetail } from "../bindings";
  import { closeDetail } from "../stores/detail.svelte";
  import type { Target } from "../stores/detail.svelte";

  /** The pod panel: containers, logs, terminal and port-forward on top of
   *  the generic YAML editor. Every other kind uses `GenericDetail`. */
  let { target }: { target: Target } = $props();
  const pod = $derived({ namespace: target.namespace, name: target.name });

  let info = $state.raw<PodDetail | null>(null);
  let loadError = $state("");
  /** Details finished loading (successfully or not): logs may start. */
  let ready = $state(false);
  let terminalOpen = $state(false);

  onMount(async () => {
    try {
      info = await commands.cmdGetPodDetails(pod.namespace, pod.name);
    } catch (e) {
      loadError = String(e);
    }
    ready = true;
  });
</script>

<aside>
  <div class="header">
    <span class="title">{pod.namespace}/{pod.name}</span>
    <button type="button" class="close" aria-label="Close details" onclick={closeDetail}>✕</button>
  </div>

  <div class="body">
    {#if loadError}
      <p class="error-text">Could not load pod details: {loadError}</p>
    {:else if !info}
      <p class="muted">Loading…</p>
    {:else}
      <dl class="meta">
        <dt>Phase</dt>
        <dd class="phase-{info.phase}">{info.phase}</dd>
        <dt>Node</dt>
        <dd>{info.node}</dd>
        <dt>IP</dt>
        <dd>{info.ip || "—"}</dd>
        <dt>Age</dt>
        <dd>{info.age}</dd>
      </dl>
      {#if Object.keys(info.labels).length > 0}
        <div class="labels">
          {#each Object.entries(info.labels) as [k, v] (k)}
            <span class="label-chip">{k}={v}</span>
          {/each}
        </div>
      {/if}
      <h3>Containers</h3>
      <div class="scroll-x">
        <table>
          <thead><tr><th>Name</th><th>Image</th><th>State</th><th>Restarts</th></tr></thead>
          <tbody>
            {#each info.containers as c (c.name)}
              <tr><td>{c.name}</td><td>{c.image}</td><td>{c.state}</td><td>{c.restarts}</td></tr>
            {:else}
              <tr><td colspan="4">No containers.</td></tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </div>

  <LogViewer {pod} containers={info?.log_containers ?? []} {ready} />

  <div class="section-header">
    <h3>Terminal</h3>
    {#if terminalOpen}
      <button type="button" class="btn" onclick={() => (terminalOpen = false)}>Close</button>
    {:else}
      <button type="button" class="btn" onclick={() => (terminalOpen = true)}>Open terminal</button>
    {/if}
  </div>
  {#if terminalOpen}
    <Terminal {pod} container={info?.containers[0]?.name ?? null} />
  {/if}

  <PortForward {pod} />
  <YamlEditor {target} />
</aside>

<style>
  aside {
    width: 420px;
    flex-shrink: 0;
    background: var(--bg-raised);
    border-left: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow-y: auto;
  }
  /* Sections keep their natural height and the panel scrolls as a whole;
     without this the flex column squeezes them. */
  aside > :global(*) {
    flex-shrink: 0;
  }
  .header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border);
    font-weight: 600;
    font-size: 13px;
    word-break: break-all;
  }
  .close {
    background: transparent;
    color: var(--text-muted);
    border: none;
    font-size: 14px;
    cursor: pointer;
    flex-shrink: 0;
  }
  .close:hover {
    color: var(--text);
  }
  .body {
    padding: 12px 16px;
    overflow: auto;
  }
  .body h3 {
    margin: 14px 0 8px;
    font-size: 13px;
  }
  .body table {
    font-size: 12px;
  }
  .scroll-x {
    overflow-x: auto;
  }
  dl.meta {
    display: grid;
    grid-template-columns: 70px 1fr;
    gap: 4px 12px;
    margin: 0;
    font-size: 13px;
  }
  dl.meta dt {
    color: var(--text-muted);
  }
  dl.meta dd {
    margin: 0;
  }
  .labels {
    margin-top: 10px;
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .label-chip {
    background: var(--bg-hover);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 2px 6px;
    font-size: 12px;
    word-break: break-all;
  }
</style>
