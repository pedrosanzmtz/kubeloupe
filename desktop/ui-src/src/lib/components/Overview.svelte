<script lang="ts">
  import { onMount } from "svelte";
  import { commands } from "../bindings";
  import type { Overview } from "../bindings";
  import { app, setStatus, showError, stamp } from "../stores/app.svelte";

  /** The overview is not watched; it re-requests itself while it is open. */
  const REFRESH_MS = 30_000;

  let overview = $state.raw<Overview | null>(null);

  async function load() {
    showError("");
    try {
      const ov = await commands.cmdGetOverview();
      setStatus(true, "Connected");
      overview = ov;
      app.count = `${ov.pods_total} pods, ${ov.nodes} nodes`;
      stamp();
    } catch (e) {
      setStatus(false, "Cluster unreachable");
      showError("Could not reach the cluster: " + e);
      overview = null;
    }
  }

  // Runs on mount and whenever the Refresh button or a context switch bumps the nonce.
  $effect(() => {
    void app.refreshNonce;
    load();
  });

  onMount(() => {
    const timer = setInterval(load, REFRESH_MS);
    return () => clearInterval(timer);
  });
</script>

<div class="overview">
  {#if overview}
    <div class="cards">
      <div class="card">
        <div class="num">{overview.namespaces}</div>
        <div class="label">Namespaces</div>
      </div>
      <div class="card">
        <div class="num">{overview.nodes_ready}/{overview.nodes}</div>
        <div class="label">Nodes ready</div>
      </div>
      <div class="card">
        <div class="num">{overview.pods_total}</div>
        <div class="label">Pods</div>
      </div>
      <div class="card">
        <div class="num">{overview.pods_running}</div>
        <div class="label">Running</div>
      </div>
      <div class="card">
        <div class="num">{overview.pods_pending}</div>
        <div class="label">Pending</div>
      </div>
      <div class="card">
        <div class="num">{overview.pods_failed}</div>
        <div class="label">Failed</div>
      </div>
    </div>
    <div class="sections">
      <section>
        <h3>Pods by namespace</h3>
        <table>
          <thead><tr><th>Namespace</th><th>Pods</th></tr></thead>
          <tbody>
            {#each overview.pods_by_namespace as [ns, n] (ns)}
              <tr><td>{ns}</td><td>{n}</td></tr>
            {/each}
          </tbody>
        </table>
      </section>
      <section>
        <h3>Recent events</h3>
        {#each overview.events as e, i (i)}
          <div class="event" class:warning={e.kind === "Warning"}>
            <span class="age">{e.age}</span>
            <span class="reason">{e.reason}</span>
            <span class="object">{e.object}</span>
            <span class="message">{e.message}{e.count > 1 ? ` (x${e.count})` : ""}</span>
          </div>
        {:else}
          <p class="muted">No events.</p>
        {/each}
      </section>
    </div>
  {/if}
</div>

<style>
  .overview {
    flex: 1;
    overflow: auto;
    padding: 16px;
  }
  .cards {
    display: grid;
    grid-template-columns: repeat(6, 1fr);
    gap: 12px;
    margin-bottom: 20px;
  }
  .card {
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 14px 16px;
  }
  .num {
    font-size: 24px;
    font-weight: 700;
  }
  .label {
    color: var(--text-muted);
    font-size: 12px;
    margin-top: 2px;
  }
  .sections {
    display: grid;
    grid-template-columns: 1fr 2fr;
    gap: 24px;
  }
  h3 {
    margin: 0 0 10px;
    font-size: 14px;
  }
  .event {
    display: flex;
    gap: 12px;
    padding: 8px 10px;
    border-bottom: 1px solid var(--border);
    font-size: 13px;
    align-items: baseline;
  }
  .event.warning {
    border-left: 3px solid var(--amber);
  }
  .age {
    color: var(--text-muted);
    min-width: 32px;
  }
  .reason {
    font-weight: 600;
    min-width: 110px;
  }
  .object {
    color: var(--text-muted);
    min-width: 180px;
  }
  .message {
    flex: 1;
  }
</style>
