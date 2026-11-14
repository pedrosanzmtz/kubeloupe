<script lang="ts">
  import { app, refresh, switchContext } from "../stores/app.svelte";
  import { live } from "../stores/live.svelte";
  import { openSettings } from "../stores/keyboard.svelte";
  import { confirmDiscard } from "../stores/unsaved.svelte";

  async function onContextChange(select: HTMLSelectElement) {
    const name = select.value;
    if (!(await confirmDiscard())) {
      select.value = current;
      return;
    }
    // The backend drops its watches on a context switch; stop ours first.
    live.stop();
    return switchContext(name);
  }

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
    <span data-testid="connection-status">{app.status.text}</span>
  </div>
  <select
    class="select"
    class:invisible={app.contexts.length <= 1}
    aria-label="Cluster context"
    value={current}
    onchange={(e) => onContextChange(e.currentTarget)}
  >
    {#each app.contexts as ctx (ctx.name)}
      <option value={ctx.name}>{ctx.name}</option>
    {/each}
  </select>
  <button class="refresh" type="button" onclick={refresh}>Refresh</button>
  <button class="gear" type="button" aria-label="Settings" title="Settings" onclick={openSettings}>
    <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">
      <path
        fill="currentColor"
        d="M19.14 12.94a7.07 7.07 0 0 0 0-1.88l2.03-1.58a.5.5 0 0 0 .12-.64l-1.92-3.32a.5.5 0 0 0-.6-.22l-2.39.96a7.03 7.03 0 0 0-1.63-.94l-.36-2.54A.5.5 0 0 0 13.9 2h-3.84a.5.5 0 0 0-.49.42l-.36 2.54c-.59.24-1.13.56-1.63.94l-2.39-.96a.5.5 0 0 0-.6.22L2.67 8.48a.5.5 0 0 0 .12.64l2.03 1.58a7.07 7.07 0 0 0 0 1.88l-2.03 1.58a.5.5 0 0 0-.12.64l1.92 3.32c.13.22.39.3.6.22l2.39-.96c.5.38 1.04.7 1.63.94l.36 2.54c.04.24.25.42.49.42h3.84c.24 0 .45-.18.49-.42l.36-2.54c.59-.24 1.13-.56 1.63-.94l2.39.96c.22.08.47 0 .6-.22l1.92-3.32a.5.5 0 0 0-.12-.64l-2.03-1.58zM12 15.5a3.5 3.5 0 1 1 0-7 3.5 3.5 0 0 1 0 7z"
      />
    </svg>
  </button>
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
  .gear {
    display: flex;
    align-items: center;
    background: transparent;
    border: none;
    color: var(--text-muted);
    padding: 4px;
    border-radius: 6px;
    cursor: pointer;
  }
  .gear:hover {
    color: var(--text);
    background: var(--bg-hover);
  }
</style>
