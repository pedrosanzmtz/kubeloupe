<script lang="ts">
  import { kindOfResource, requestAction } from "../stores/actions.svelte";
  import type { Target } from "../stores/detail.svelte";

  /** Buttons for every action the registry lists for the object's kind. The
   *  context menu in the table reads the same list. `disabled` once the
   *  object is gone. */
  let { target, disabled = false }: { target: Target; disabled?: boolean } = $props();

  const kind = $derived(kindOfResource(target.resource));
</script>

{#if kind && kind.actions.length > 0}
  <div class="bar" role="toolbar" aria-label="Actions">
    {#each kind.actions as a (a.id)}
      <button
        type="button"
        class="btn"
        class:danger={a.destructive}
        {disabled}
        onclick={() => requestAction(kind, [{ namespace: target.namespace, name: target.name }], a)}
      >
        {a.label}
      </button>
    {/each}
  </div>
{/if}

<style>
  .bar {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 8px 16px;
    border-bottom: 1px solid var(--border);
  }
  .btn.danger {
    border-color: var(--red);
    color: var(--error-text);
  }
  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
