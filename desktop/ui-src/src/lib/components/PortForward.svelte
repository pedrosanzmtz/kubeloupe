<script lang="ts">
  import { onDestroy } from "svelte";
  import type { ForwardTarget } from "../bindings";
  import { parseLocalPort, parseRemotePort, PORT_FORWARDS_VIEW, sameTarget } from "../forwards";
  import type { PodRef } from "../stores/detail.svelte";
  import { forwards, openForward, pollForwards, startForward, stopForward } from "../stores/forwards.svelte";
  import { setView } from "../stores/app.svelte";

  /** Port-forwards of this pod. They run in the backend: closing the panel
   *  leaves them running (Network → Port Forwards lists them all). */
  let { pod, disabled = false }: { pod: PodRef; disabled?: boolean } = $props();

  const target = $derived<ForwardTarget>({ kind: "pod", namespace: pod.namespace, name: pod.name });
  const mine = $derived(forwards.list.filter((f) => sameTarget(f.target, target)));

  let port = $state("80");
  let local = $state("");
  let status = $state("");

  const stopPolling = pollForwards();
  onDestroy(stopPolling);

  async function start(remoteText: string, localText: string) {
    const remote = parseRemotePort(remoteText);
    const loc = parseLocalPort(localText);
    if (!remote.ok || !loc.ok) {
      status = !remote.ok ? remote.error : !loc.ok ? loc.error : "";
      return;
    }
    status = "Starting…";
    try {
      const f = await startForward(target, remote.value, loc.value);
      status = `Forwarding ${f.url} → ${pod.name}:${f.remote}`;
    } catch (e) {
      status = "Could not start port-forward: " + e;
    }
  }

  /** Forward `p` (from a container card's port list) on an automatic local port. */
  export function forward(p: number) {
    port = String(p);
    void start(String(p), "");
  }
</script>

<div class="section-header">
  <h3>Port forward</h3>
  <input class="port input" aria-label="Pod port" placeholder="port" bind:value={port} />
  <input class="port input" aria-label="Local port" placeholder="local" bind:value={local} />
  <button type="button" class="btn" {disabled} onclick={() => start(port, local)}>Forward</button>
</div>
{#if status}<p class="detail-status muted">{status}</p>{/if}
{#if mine.length > 0}
  <ul class="forwards">
    {#each mine as f (f.id)}
      <li>
        <span class="mono">{f.url} → {f.remote}</span>
        {#if f.status.last_error}<span class="error-text">{f.status.last_error}</span>{/if}
        <span class="spacer"></span>
        <button type="button" class="btn" aria-label="Open {f.url}" onclick={() => openForward(f)}>Open</button>
        <button type="button" class="btn" aria-label="Stop {f.url}" onclick={() => stopForward(f.id)}>Stop</button>
      </li>
    {/each}
  </ul>
  <p class="detail-status muted">
    Forwards keep running after you close this panel.
    <button type="button" class="link" onclick={() => setView(PORT_FORWARDS_VIEW)}>All port forwards</button>
  </p>
{/if}

<style>
  .port {
    width: 72px;
    padding: 4px 8px;
    font-size: 12px;
  }
  .forwards {
    list-style: none;
    margin: 0;
    padding: 0 16px;
    font-size: 12px;
  }
  .forwards li {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 3px 0;
    flex-wrap: wrap;
  }
  .spacer {
    flex: 1;
  }
  .mono {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
  }
  .link {
    background: none;
    border: none;
    padding: 0;
    color: var(--accent);
    font-size: 12px;
    cursor: pointer;
  }
</style>
