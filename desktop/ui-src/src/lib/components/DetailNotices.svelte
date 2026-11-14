<script lang="ts">
  import type { LiveDetail } from "../stores/live-detail.svelte";

  /** "This <kind> was deleted" and live-update problems, above a panel's body. */
  let { live, kind }: { live: LiveDetail; kind: string } = $props();
</script>

{#if live.deleted}
  <p class="notice deleted" role="status">This {kind} was deleted. Showing its last known state.</p>
{:else if live.watchError && live.info}
  <p class="notice" role="status">Live updates interrupted, retrying: {live.watchError}</p>
{/if}

<style>
  .notice {
    margin: 0;
    padding: 8px 16px;
    font-size: 12px;
    border-bottom: 1px solid var(--border);
    color: var(--amber);
  }
  .deleted {
    color: var(--red);
    font-weight: 600;
  }
</style>
