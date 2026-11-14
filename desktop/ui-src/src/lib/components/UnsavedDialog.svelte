<script lang="ts">
  import { answerDiscard, unsaved } from "../stores/unsaved.svelte";

  /** "Keep editing" is the safe default: focused, and what Escape does. */
  function focus(node: HTMLElement) {
    node.focus();
  }

  function onkeydown(e: KeyboardEvent) {
    if (unsaved.asking && e.key === "Escape") answerDiscard(false);
  }
</script>

<svelte:window {onkeydown} />

{#if unsaved.asking}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="backdrop" onclick={(e) => e.target === e.currentTarget && answerDiscard(false)}>
    <div class="dialog" role="alertdialog" aria-modal="true" aria-label="Discard changes?">
      <h2>Discard changes?</h2>
      <p>You have edits that are not saved. They will be lost.</p>
      <div class="buttons">
        <button type="button" class="btn" use:focus onclick={() => answerDiscard(false)}>Keep editing</button>
        <button type="button" class="btn danger" onclick={() => answerDiscard(true)}>Discard changes</button>
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
    z-index: 110;
  }
  .dialog {
    width: min(400px, calc(100vw - 32px));
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 18px 20px;
  }
  h2 {
    margin: 0 0 10px;
    font-size: 15px;
  }
  p {
    margin: 0 0 8px;
    font-size: 13px;
  }
  .buttons {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 12px;
  }
  .danger {
    border-color: var(--red);
    color: #f5a3a5;
  }
</style>
