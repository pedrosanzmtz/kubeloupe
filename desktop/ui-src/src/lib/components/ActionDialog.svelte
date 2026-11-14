<script lang="ts">
  import { confirmCopy, isDestructive, listTargets } from "../actions";
  import { actionUi, canConfirm, closeDialog, confirmRequest } from "../stores/actions.svelte";
  import ForwardDialog from "./ForwardDialog.svelte";
  import DebugDialog from "./DebugDialog.svelte";

  const req = $derived(actionUi.request);
  const copy = $derived(
    req ? confirmCopy(req.action, req.kind.resource.kind, req.targets.length, req.replicas) : null,
  );
  const danger = $derived(req ? isDestructive(req.action, req.replicas) : false);
  const drain = $derived(actionUi.drain);
  const failures = $derived(actionUi.failures);

  function onkeydown(e: KeyboardEvent) {
    if (!req) return;
    if (e.key === "Escape") closeDialog();
  }

  /** Focus the first field when the dialog opens. */
  function focusFirst(node: HTMLElement) {
    node.querySelector<HTMLElement>("input")?.focus();
  }
</script>

<svelte:window {onkeydown} />
<ForwardDialog />
<DebugDialog />

{#if req && copy}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="backdrop" onclick={(e) => e.target === e.currentTarget && closeDialog()}>
    <div class="dialog" role="alertdialog" data-testid="action-dialog" aria-modal="true" aria-label={copy.title} use:focusFirst>
      <h2 class:danger>{copy.title}</h2>

      {#if drain}
        <p class="lead">Draining <strong>{drain.node}</strong></p>
        <ul class="log" aria-live="polite">
          {#each drain.lines as line, i (i)}
            <li class:bad={line.includes("failed") || line.includes("blocked")}>{line}</li>
          {/each}
          {#if !drain.finished}<li class="muted">Working…</li>{/if}
        </ul>
        {#if drain.error}<p class="error-text" role="alert">{drain.error}</p>{/if}
        {#if drain.summary}
          <p class={drain.summary.failed || drain.summary.still_terminating ? "error-text" : "ok"}>
            {drain.summary.evicted} evicted, {drain.summary.failed} failed, {drain.summary.skipped} skipped{drain.summary
              .still_terminating
              ? `, ${drain.summary.still_terminating} still terminating`
              : ""}.
            {#if !drain.summary.failed && !drain.summary.still_terminating}The node stays cordoned until you
              uncordon it.{/if}
          </p>
        {/if}
        <div class="buttons">
          <button type="button" class="btn" onclick={closeDialog}>{drain.finished ? "Close" : "Stop watching"}</button>
        </div>
      {:else if failures.length > 0}
        <p class="lead">
          {actionUi.deleted} of {req.targets.length} deleted. {failures.length} could not be deleted:
        </p>
        <ul class="targets">
          {#each failures as f (f.key)}
            <li><span class="mono">{f.key}</span> <span class="error-text">{f.error}</span></li>
          {/each}
        </ul>
        <div class="buttons">
          <button type="button" class="btn" onclick={closeDialog}>Close</button>
        </div>
      {:else}
        <p class="lead">{copy.lead}</p>
        <ul class="targets">
          {#each listTargets(req.targets) as name, i (i)}
            <li class="mono">{name}</li>
          {/each}
        </ul>

        {#if req.action.input === "replicas"}
          <label class="field">
            Replicas
            <input
              class="input"
              type="number"
              data-testid="action-replicas"
              min="0"
              step="1"
              bind:value={req.replicas}
              onkeydown={(e) => e.key === "Enter" && confirmRequest()}
            />
          </label>
        {/if}

        {#if copy.warning}<p class="warning">{copy.warning}</p>{/if}

        {#if req.action.type_to_confirm}
          <label class="field">
            Type <span class="mono">{req.targets[0].name}</span> to confirm
            <input
              class="input mono"
              bind:value={req.typed}
              autocomplete="off"
              spellcheck="false"
              onkeydown={(e) => e.key === "Enter" && confirmRequest()}
            />
          </label>
        {/if}

        {#if actionUi.error}<p class="error-text" role="alert">{actionUi.error}</p>{/if}

        <div class="buttons">
          <button type="button" class="btn" disabled={actionUi.busy} onclick={closeDialog}>Cancel</button>
          <button
            type="button"
            class="btn confirm"
            data-testid="action-confirm"
            class:danger
            disabled={!canConfirm(req)}
            onclick={confirmRequest}
          >
            {actionUi.busy ? "Working…" : copy.button}
          </button>
        </div>
      {/if}
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
    max-height: calc(100vh - 64px);
    overflow: auto;
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
  .lead {
    margin: 0 0 8px;
    font-size: 13px;
  }
  .targets,
  .log {
    list-style: none;
    margin: 0 0 12px;
    padding: 8px 10px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    max-height: 200px;
    overflow: auto;
    font-size: 12px;
  }
  .targets li,
  .log li {
    padding: 1px 0;
    word-break: break-all;
  }
  .log li.bad {
    color: var(--amber);
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
  .warning {
    margin: 0 0 12px;
    padding: 8px 10px;
    border: 1px solid var(--amber);
    border-radius: 6px;
    font-size: 12px;
    color: var(--amber);
  }
  .ok {
    color: var(--green);
    font-size: 13px;
  }
  .buttons {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 8px;
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
