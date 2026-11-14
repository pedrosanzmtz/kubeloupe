<script lang="ts">
  import type { ConditionInfo } from "../bindings";

  /** `status.conditions` of any object; the message is the row's tooltip. */
  let { conditions }: { conditions: ConditionInfo[] } = $props();
</script>

{#if conditions.length > 0}
  <h3>Conditions</h3>
  <div class="scroll-x">
    <table class="conditions">
      <thead><tr><th>Type</th><th>Status</th><th>Reason</th><th>Since</th></tr></thead>
      <tbody>
        {#each conditions as c (c.type)}
          <tr title={c.message}>
            <td>{c.type}</td>
            <td class:ok={c.status === "True"} class:bad={c.status === "False"}>{c.status}</td>
            <td>{c.reason}</td>
            <td>{c.age}</td>
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
  .scroll-x {
    overflow-x: auto;
  }
  .ok {
    color: var(--green);
  }
  .bad {
    color: var(--amber);
  }
</style>
