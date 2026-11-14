<script lang="ts">
  import { app } from "../../stores/app.svelte";
  import { canConfirmHelm, closeHelmDialog, confirmHelmAction, helm } from "../../stores/helm.svelte";

  /** Confirmation for a Helm rollback or uninstall (run through the helm CLI). */
  const req = $derived(helm.request);
  const title = $derived(
    req?.action === "rollback" ? `Roll back ${req.name} to revision ${req.revision}?` : `Uninstall ${req?.name}?`,
  );

  function confirm() {
    void confirmHelmAction(app.selectedNamespaces);
  }

  function onkeydown(e: KeyboardEvent) {
    if (req && e.key === "Escape") closeHelmDialog();
  }

  function focusFirst(node: HTMLElement) {
    (node.querySelector<HTMLElement>("input") ?? node.querySelector<HTMLElement>("button.confirm"))?.focus();
  }
</script>

<svelte:window {onkeydown} />

{#if req}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="backdrop" onclick={(e) => e.target === e.currentTarget && closeHelmDialog()}>
    <div class="dialog" role="alertdialog" aria-modal="true" aria-label={title} data-testid="helm-dialog" use:focusFirst>
      <h2 class:danger={req.action === "uninstall"}>{title}</h2>
      {#if req.action === "rollback"}
        <p class="lead">
          Helm creates revision {req.current + 1} of <span class="mono">{req.namespace}/{req.name}</span> with the values
          and manifest of revision {req.revision}, and applies it to the cluster.
        </p>
      {:else}
        <p class="lead">
          Helm deletes every object of <span class="mono">{req.namespace}/{req.name}</span> and the release's history.
          This cannot be undone.
        </p>
        <label class="field">
          Type <span class="mono">{req.name}</span> to confirm
          <input
            class="input mono"
            bind:value={req.typed}
            autocomplete="off"
            spellcheck="false"
            onkeydown={(e) => e.key === "Enter" && confirm()}
          />
        </label>
      {/if}
      {#if helm.helmMissing}<p class="error-text">{helm.helmMissing}</p>{/if}
      {#if helm.actionError}<p class="error-text" role="alert">{helm.actionError}</p>{/if}
      <div class="buttons">
        <button type="button" class="btn" disabled={helm.busy} onclick={closeHelmDialog}>Cancel</button>
        <button
          type="button"
          class="btn confirm"
          class:danger={req.action === "uninstall"}
          data-testid="helm-confirm"
          disabled={!canConfirmHelm(req)}
          onclick={confirm}
        >
          {helm.busy ? "Working…" : req.action === "rollback" ? "Roll back" : "Uninstall"}
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
  }
  .dialog {
    width: min(460px, calc(100vw - 32px));
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 18px 20px;
  }
  h2 {
    margin: 0 0 10px;
    font-size: 15px;
  }
  h2.danger {
    color: #f5a3a5;
  }
  .lead,
  .error-text {
    margin: 0 0 12px;
    font-size: 13px;
  }
  .mono {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-bottom: 12px;
    font-size: 13px;
  }
  .buttons {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .confirm.danger {
    border-color: var(--red);
    color: #f5a3a5;
  }
  .btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
</style>
