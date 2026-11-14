<script lang="ts">
  import { actionUi, dismissNotice } from "../stores/actions.svelte";
</script>

{#if actionUi.notice}
  <div class="notice" class:fail={!actionUi.notice.ok} role={actionUi.notice.ok ? "status" : "alert"}>
    <span>{actionUi.notice.text}</span>
    <button type="button" class="x" aria-label="Dismiss" onclick={dismissNotice}>✕</button>
  </div>
{/if}

<style>
  .notice {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    margin: 12px 16px 0;
    padding: 8px 12px;
    background: rgba(63, 178, 127, 0.12);
    border: 1px solid var(--green);
    border-radius: 6px;
    font-size: 13px;
  }
  .notice.fail {
    background: rgba(229, 72, 77, 0.15);
    border-color: var(--red);
    color: #f5a3a5;
  }
  .x {
    background: transparent;
    border: none;
    color: inherit;
    cursor: pointer;
  }
</style>
