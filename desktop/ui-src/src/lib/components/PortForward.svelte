<script lang="ts">
  import { onDestroy } from "svelte";
  import { commands } from "../bindings";
  import type { PodRef } from "../stores/detail.svelte";

  let { pod, disabled = false }: { pod: PodRef; disabled?: boolean } = $props();

  let port = $state(80);
  let status = $state("");
  let session: string | null = $state(null);

  async function start() {
    const p = Math.trunc(Number(port));
    if (!p || p < 1 || p > 65535) {
      status = "Enter a port between 1 and 65535.";
      return;
    }
    status = "Starting…";
    try {
      const info = await commands.cmdPortforwardStart(pod.namespace, pod.name, p);
      session = info.session;
      status = `Forwarding 127.0.0.1:${info.local_port} → ${pod.name}:${p}`;
    } catch (e) {
      status = "Could not start port-forward: " + e;
    }
  }

  function stop() {
    if (session) commands.cmdPortforwardStop(session).catch(() => {});
    session = null;
    status = "";
  }

  /** Forward `p` (from a container card's port list), replacing any forward. */
  export function forward(p: number) {
    stop();
    port = p;
    void start();
  }

  onDestroy(stop);
</script>

<div class="section-header">
  <h3>Port forward</h3>
  <input
    class="port"
    type="number"
    min="1"
    max="65535"
    aria-label="Pod port"
    bind:value={port}
  />
  {#if session}
    <button type="button" class="btn" onclick={stop}>Stop</button>
  {:else}
    <button type="button" class="btn" {disabled} onclick={start}>Forward</button>
  {/if}
</div>
<p class="detail-status muted">{status}</p>

<style>
  .port {
    width: 80px;
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 4px 8px;
    font-size: 12px;
  }
</style>
