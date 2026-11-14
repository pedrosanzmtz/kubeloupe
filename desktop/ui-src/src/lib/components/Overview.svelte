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
      // Parts the user may not see cluster-wide fall back to this namespace.
      const ov = await commands.cmdGetOverview(app.namespace || null);
      setStatus(true, "Connected");
      overview = ov;
      const counts = [];
      if (ov.pods) counts.push(`${ov.pods.total} pods`);
      if (ov.nodes) counts.push(`${ov.nodes.total} nodes`);
      app.count = counts.join(", ");
      stamp();
    } catch (e) {
      setStatus(false, "Cluster unreachable");
      showError("Could not reach the cluster: " + e);
      overview = null;
    }
  }

  // Runs on mount and whenever the Refresh button or a context switch bumps the nonce.
  // The namespace matters only to users who cannot see the whole cluster.
  $effect(() => {
    void app.refreshNonce;
    void app.namespace;
    load();
  });

  onMount(() => {
    const timer = setInterval(load, REFRESH_MS);
    return () => clearInterval(timer);
  });
</script>

<div class="overview">
  {#if overview}
    {@const pods = overview.pods}
    {#if overview.notes.length > 0}
      <ul class="notes muted">
        {#each overview.notes as note (note)}
          <li>{note}</li>
        {/each}
      </ul>
    {/if}
    <div class="cards">
      {#if overview.namespaces !== null}
        <div class="card">
          <div class="num">{overview.namespaces}</div>
          <div class="label">Namespaces</div>
        </div>
      {/if}
      {#if overview.nodes}
        <div class="card">
          <div class="num">{overview.nodes.ready}/{overview.nodes.total}</div>
          <div class="label">Nodes ready</div>
        </div>
      {/if}
      {#if pods}
        {@const scope = pods.namespace ? ` in ${pods.namespace}` : ""}
        <div class="card">
          <div class="num">{pods.total}</div>
          <div class="label">Pods{scope}</div>
        </div>
        <div class="card">
          <div class="num">{pods.running}</div>
          <div class="label">Running</div>
        </div>
        <div class="card">
          <div class="num">{pods.pending}</div>
          <div class="label">Pending</div>
        </div>
        <div class="card">
          <div class="num">{pods.failed}</div>
          <div class="label">Failed</div>
        </div>
      {/if}
    </div>
    <div class="sections">
      <section>
        <h3>Pods by namespace</h3>
        {#if pods}
          <table>
            <thead><tr><th>Namespace</th><th>Pods</th></tr></thead>
            <tbody>
              {#each pods.by_namespace as [ns, n] (ns)}
                <tr><td>{ns}</td><td>{n}</td></tr>
              {/each}
            </tbody>
          </table>
        {:else}
          <p class="muted">Not allowed.</p>
        {/if}
      </section>
      <section>
        <h3>
          Recent events{overview.events?.namespace ? ` in ${overview.events.namespace}` : ""}
        </h3>
        {#each overview.events?.items ?? [] as e, i (i)}
          <div class="event" class:warning={e.kind === "Warning"}>
            <span class="age">{e.age}</span>
            <span class="reason">{e.reason}</span>
            <span class="object">{e.object}</span>
            <span class="message">{e.message}{e.count > 1 ? ` (x${e.count})` : ""}</span>
          </div>
        {:else}
          <p class="muted">{overview.events ? "No events." : "Not allowed."}</p>
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
  .notes {
    margin: 0 0 16px;
    padding-left: 18px;
    font-size: 12px;
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
