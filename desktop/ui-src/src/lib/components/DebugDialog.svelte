<script lang="ts">
  import { DEBUG_IMAGES, EPHEMERAL_NOTE } from "../debug";
  import { canStartDebug, closeDebugDialog, confirmDebugDialog, debugDialog } from "../stores/debug.svelte";

  /** Add an ephemeral debug container to a pod: which container's processes
   *  it sees, and which image it runs. The shell opens in a dock tab. */
  const pod = $derived(debugDialog.pod);

  function onkeydown(e: KeyboardEvent) {
    if (pod && e.key === "Escape") closeDebugDialog();
  }

  function onEnter(e: KeyboardEvent) {
    if (e.key === "Enter") confirmDebugDialog();
  }

  function focusFirst(node: HTMLElement) {
    node.querySelector<HTMLElement>("input, select")?.focus();
  }
</script>

<svelte:window {onkeydown} />

{#if pod}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="backdrop" onclick={(e) => e.target === e.currentTarget && closeDebugDialog()}>
    <div class="dialog" role="dialog" data-testid="debug-dialog" aria-modal="true" aria-label="Debug pod" use:focusFirst>
      <h2>Debug pod</h2>
      <p class="lead mono">{pod.namespace}/{pod.name}</p>

      <label class="field">
        Target container
        <select class="select" aria-label="Target container" bind:value={debugDialog.target}>
          {#if debugDialog.containers.length === 0}
            <option value="">First container</option>
          {/if}
          {#each debugDialog.containers as c (c)}
            <option value={c}>{c}</option>
          {/each}
        </select>
        <span class="hint muted">The debug container sees this container's processes (<span class="mono">ps</span>).</span>
      </label>

      <label class="field">
        Image
        <input
          class="input mono"
          aria-label="Debug image"
          list="debug-images"
          autocomplete="off"
          spellcheck="false"
          bind:value={debugDialog.image}
          onkeydown={onEnter}
        />
        <datalist id="debug-images">
          {#each DEBUG_IMAGES as img (img)}<option value={img}></option>{/each}
        </datalist>
        <span class="hint muted">It needs a shell. nicolaka/netshoot has network tools.</span>
      </label>

      <p class="warning">{EPHEMERAL_NOTE}</p>

      <div class="buttons">
        <button type="button" class="btn" onclick={closeDebugDialog}>Cancel</button>
        <button
          type="button"
          class="btn primary"
          data-testid="debug-confirm"
          disabled={!canStartDebug()}
          onclick={confirmDebugDialog}>Debug</button
        >
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
    width: min(440px, calc(100vw - 32px));
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
  .lead {
    margin: 0 0 12px;
    font-size: 13px;
    word-break: break-all;
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
  .hint {
    font-size: 12px;
  }
  .warning {
    margin: 0 0 12px;
    padding: 8px 10px;
    border: 1px solid var(--amber);
    border-radius: 6px;
    font-size: 12px;
    color: var(--amber);
  }
  .buttons {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 8px;
  }
  .btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
</style>
