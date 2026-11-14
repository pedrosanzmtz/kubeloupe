<script lang="ts">
  import type { RelatedEvent } from "../bindings";

  /** Events about one object, newest first. */
  let { events }: { events: RelatedEvent[] } = $props();
</script>

<h3>Events</h3>
{#if events.length === 0}
  <p class="muted small">No events.</p>
{:else}
  <div class="scroll-x">
    <table class="events">
      <thead><tr><th>Type</th><th>Reason</th><th>Message</th><th>Age</th></tr></thead>
      <tbody>
        {#each events as e, i (i)}
          <tr>
            <td class:warn={e.type === "Warning"}>{e.type}</td>
            <td>{e.reason}{e.count > 1 ? ` ×${e.count}` : ""}</td>
            <td class="wrap">{e.message}</td>
            <td>{e.age}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}

<style>
  h3 {
    margin: 14px 0 8px;
    font-size: 13px;
  }
  table {
    font-size: 12px;
  }
  td.wrap {
    white-space: normal;
    word-break: break-word;
  }
  .scroll-x {
    overflow-x: auto;
  }
  .small {
    font-size: 12px;
    margin: 0 0 6px;
  }
  .warn {
    color: var(--amber);
  }
</style>
