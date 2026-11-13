<script lang="ts">
  import { app, refresh, switchContext } from "../stores/app.svelte";

  const current = $derived(app.contexts.find((c) => c.current)?.name ?? "");
</script>

<header>
  <div class="brand">kubeloupe</div>
  <div class="cluster">
    <span
      class="dot"
      class:ok={app.status.ok === true}
      class:error={app.status.ok === false}
      aria-hidden="true"
    ></span>
    <span>{app.status.text}</span>
  </div>
  <select
    class="select"
    class:invisible={app.contexts.length <= 1}
    aria-label="Cluster context"
    value={current}
    onchange={(e) => switchContext(e.currentTarget.value)}
  >
    {#each app.contexts as ctx (ctx.name)}
      <option value={ctx.name}>{ctx.name}</option>
    {/each}
  </select>
  <button class="refresh" type="button" onclick={refresh}>Refresh</button>
</header>

<style>
  header {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 0 16px;
    height: 48px;
    background: var(--bg-raised);
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }
  .brand {
    font-weight: 700;
    font-size: 16px;
    letter-spacing: 0.02em;
  }
  .cluster {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text-muted);
  }
  .dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--amber);
  }
  .dot.ok {
    background: var(--green);
  }
  .dot.error {
    background: var(--red);
  }
  .invisible {
    visibility: hidden;
  }
  .refresh {
    margin-left: auto;
    background: var(--accent);
    color: #fff;
    border: none;
    border-radius: 6px;
    padding: 6px 14px;
    font-size: 13px;
    cursor: pointer;
  }
  .refresh:hover {
    opacity: 0.9;
  }
</style>
