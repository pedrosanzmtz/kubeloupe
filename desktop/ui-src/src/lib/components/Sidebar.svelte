<script lang="ts">
  import { app, setView } from "../stores/app.svelte";
  import type { View } from "../stores/app.svelte";

  const items: { view: View; label: string }[] = [
    { view: "overview", label: "Overview" },
    { view: "pods", label: "Pods" },
    { view: "namespaces", label: "Namespaces" },
    { view: "nodes", label: "Nodes" },
  ];
</script>

<nav>
  {#each items as item (item.view)}
    <button
      type="button"
      class:active={app.view === item.view}
      onclick={() => setView(item.view)}
    >
      {item.label}
    </button>
  {/each}
</nav>

<style>
  nav {
    width: 160px;
    flex-shrink: 0;
    background: var(--bg-raised);
    border-right: 1px solid var(--border);
    padding: 12px 8px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  button {
    text-align: left;
    background: transparent;
    color: var(--text-muted);
    border: none;
    border-radius: 6px;
    padding: 8px 12px;
    font-size: 14px;
    cursor: pointer;
  }
  button:hover {
    background: var(--bg-hover);
    color: var(--text);
  }
  button.active {
    background: var(--bg-hover);
    color: var(--text);
    font-weight: 600;
  }
</style>
