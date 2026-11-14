<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { commands } from "../bindings";
  import type { ForwardTarget, ServicePortInfo } from "../bindings";
  import { sameTarget } from "../forwards";
  import { forwards, openForward, pollForwards, startForward } from "../stores/forwards.svelte";
  import { setNotice } from "../stores/actions.svelte";

  /** A Service's ports, each with a Forward button. The forward picks a
   *  ready pod per connection, so it survives pod restarts. */
  let { namespace, name, disabled = false }: { namespace: string; name: string; disabled?: boolean } = $props();

  const target = $derived<ForwardTarget>({ kind: "service", namespace, name });
  let ports = $state.raw<ServicePortInfo[]>([]);
  let error = $state("");
  let busy = $state(0);

  const stopPolling = pollForwards();
  onDestroy(stopPolling);

  onMount(async () => {
    try {
      ports = await commands.cmdServicePorts(namespace, name);
    } catch (e) {
      error = String(e);
    }
  });

  function forwardOf(p: ServicePortInfo) {
    return forwards.list.find((f) => sameTarget(f.target, target) && f.remote_port === p.port);
  }

  async function forward(p: ServicePortInfo) {
    busy = p.port;
    try {
      const f = await startForward(target, String(p.port), null);
      setNotice(true, `Forwarding ${f.url} → svc/${namespace}/${name}:${p.port}`);
    } catch (e) {
      setNotice(false, `Port forward svc/${namespace}/${name}:${p.port}: ${e}`);
    } finally {
      busy = 0;
    }
  }
</script>

<h3>Ports</h3>
{#if error}
  <p class="error-text small">Could not load the ports: {error}</p>
{:else if ports.length === 0}
  <p class="muted small">No ports.</p>
{:else}
  <ul class="ports" data-testid="service-ports">
    {#each ports as p (p.port)}
      {@const f = forwardOf(p)}
      <li>
        <span class="mono">{p.port}{p.name ? `/${p.name}` : ""} → {p.target_port}</span>
        <span class="muted">{p.protocol}</span>
        <span class="spacer"></span>
        {#if f}
          <button type="button" class="btn" aria-label="Open {f.url}" onclick={() => openForward(f)}>{f.url}</button>
        {:else if p.protocol === "TCP"}
          <button
            type="button"
            class="btn"
            aria-label="Forward port {p.port}"
            disabled={disabled || busy === p.port}
            onclick={() => forward(p)}>Forward</button
          >
        {/if}
      </li>
    {/each}
  </ul>
{/if}

<style>
  h3 {
    margin: 14px 0 8px;
    font-size: 13px;
  }
  .small {
    font-size: 12px;
    margin: 0 0 6px;
  }
  .ports {
    list-style: none;
    margin: 0;
    padding: 0;
    font-size: 13px;
  }
  .ports li {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 3px 0;
  }
  .spacer {
    flex: 1;
  }
  .mono {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 12px;
  }
</style>
