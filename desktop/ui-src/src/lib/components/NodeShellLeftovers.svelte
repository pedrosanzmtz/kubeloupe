<script lang="ts">
  import { leftoverText } from "../debug";
  import { cleanUpLeftovers, dismissLeftovers, leftovers } from "../stores/debug.svelte";

  /** Offer to delete node shell pods an earlier session left behind. */
</script>

{#if leftovers.pods.length > 0}
  <div class="offer" role="status" data-testid="node-shell-leftovers">
    <span>
      {leftoverText(leftovers.pods.length)}
      <span class="mono muted">{leftovers.pods.map((p) => `${p.namespace}/${p.name}`).join(", ")}</span>
    </span>
    <span class="buttons">
      <button type="button" class="btn" disabled={leftovers.busy} onclick={() => void cleanUpLeftovers()}
        >{leftovers.busy ? "Deleting…" : "Delete"}</button
      >
      <button type="button" class="btn" disabled={leftovers.busy} onclick={dismissLeftovers}>Keep</button>
    </span>
  </div>
{/if}

<style>
  .offer {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
    margin: 12px 16px 0;
    padding: 8px 12px;
    border: 1px solid var(--amber);
    border-radius: 6px;
    font-size: 13px;
  }
  .mono {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 12px;
    word-break: break-all;
  }
  .buttons {
    display: flex;
    gap: 8px;
    flex-shrink: 0;
  }
</style>
