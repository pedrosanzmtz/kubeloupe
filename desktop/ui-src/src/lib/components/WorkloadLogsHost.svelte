<script lang="ts">
  import LogViewer from "./LogViewer.svelte";
  import { closeWorkloadLogs, workloadLogs } from "../stores/workloadLogs.svelte";
</script>

<!-- A drawer over the bottom of the window, mounted on its own root (see
     main.ts) so it does not depend on the app layout. Open it with
     `openWorkloadLogs({ kind, namespace, name })`. -->
{#if workloadLogs.target}
  {#key workloadLogs.nonce}
    <div class="drawer" role="dialog" aria-label="Workload logs">
      <div class="head">
        <span class="title">{workloadLogs.target.kind} {workloadLogs.target.namespace}/{workloadLogs.target.name}</span>
        <button type="button" class="close" aria-label="Close logs" onclick={closeWorkloadLogs}>✕</button>
      </div>
      <LogViewer workload={workloadLogs.target} />
    </div>
  {/key}
{/if}

<style>
  .drawer {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 0;
    z-index: 50;
    display: flex;
    flex-direction: column;
    max-height: 70vh;
    background: var(--bg-raised);
    border-top: 1px solid var(--border);
    box-shadow: 0 -8px 24px rgba(0, 0, 0, 0.4);
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 16px;
    border-bottom: 1px solid var(--border);
    font-size: 13px;
    font-weight: 600;
  }
  .close {
    background: transparent;
    color: var(--text-muted);
    border: none;
    font-size: 14px;
    cursor: pointer;
  }
  .close:hover {
    color: var(--text);
  }
  .drawer :global(.output) {
    height: 45vh;
  }
</style>
