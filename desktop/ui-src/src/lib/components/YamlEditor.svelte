<script lang="ts">
  import { onMount } from "svelte";
  import { commands } from "../bindings";
  import type { FieldTakeover } from "../bindings";
  import type { Target } from "../stores/detail.svelte";

  let { target }: { target: Target } = $props();

  // The panel is remounted for every object, so `target` is fixed here.
  // svelte-ignore state_referenced_locally
  const isSecret = target.resource.group === "" && target.resource.kind === "Secret";

  let text = $state("");
  let status = $state("");
  /** Secret values are hidden until the user asks for them. */
  let revealed = $state(false);
  /** Nothing to save until a load succeeded. */
  let loaded = $state(false);

  async function load() {
    text = "Loading…";
    status = "";
    loaded = false;
    try {
      text = await commands.cmdGetResourceYaml(
        target.resource,
        target.namespace,
        target.name,
        revealed,
      );
      loaded = true;
    } catch (e) {
      text = "";
      status = "Could not load YAML: " + e;
    }
  }

  /** Fields the last save took over from other field managers. */
  let takeovers = $state.raw<FieldTakeover[]>([]);

  async function save() {
    status = "Saving…";
    takeovers = [];
    try {
      const took = await commands.cmdApplyResourceYaml(
        target.resource,
        target.namespace,
        target.name,
        text,
      );
      // Reload so the editor shows what the server now holds.
      await load();
      takeovers = took;
      status = "Saved.";
    } catch (e) {
      status = "Could not save YAML: " + e;
    }
  }

  async function reveal() {
    revealed = true;
    await load();
  }

  async function hide() {
    revealed = false;
    await load();
  }

  onMount(load);

  const locked = $derived(isSecret && !revealed);
</script>

<div class="section-header">
  <h3>YAML</h3>
  {#if isSecret}
    {#if revealed}
      <button type="button" class="btn" onclick={hide}>Hide values</button>
    {:else}
      <button type="button" class="btn" onclick={reveal}>Reveal values</button>
    {/if}
  {/if}
  <button type="button" class="btn" onclick={load}>Reload</button>
  <button type="button" class="btn primary" onclick={save} disabled={locked || !loaded}>Save</button>
</div>
<textarea
  spellcheck="false"
  aria-label="{target.resource.kind} YAML"
  readonly={locked}
  bind:value={text}
></textarea>
<p class="detail-status muted">
  {status}{#if locked && !status}Values are hidden. Reveal them to edit this Secret.{/if}
</p>
{#if takeovers.length > 0}
  <div class="takeover" role="note">
    This save took over fields another tool was managing. That tool may change them back the
    next time it runs.
    <ul>
      {#each takeovers as t (t.manager)}
        <li>
          from <strong>{t.manager}</strong>:
          {#each t.fields as f, i (f)}<code>{f}</code>{i < t.fields.length - 1 ? ", " : ""}{/each}
        </li>
      {/each}
    </ul>
  </div>
{/if}

<style>
  textarea {
    width: 100%;
    min-height: 220px;
    margin: 0;
    padding: 10px 16px;
    background: #0d1013;
    color: #d6dae0;
    border: none;
    border-top: 1px solid var(--border);
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 12px;
    line-height: 1.5;
    resize: vertical;
    outline: none;
  }
  .takeover {
    margin: 0 16px 12px;
    padding: 8px 10px;
    border: 1px solid var(--amber);
    border-radius: 4px;
    font-size: 12px;
  }
  .takeover ul {
    margin: 6px 0 0;
    padding-left: 18px;
  }
  .takeover code {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    word-break: break-all;
  }
  .btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
</style>
