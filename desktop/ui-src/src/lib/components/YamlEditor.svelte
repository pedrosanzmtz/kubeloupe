<script lang="ts" module>
  /** Edits kept while the editor is unmounted (another tab of the panel,
   *  or the same object re-opened), until saved or discarded. */
  interface Draft {
    base: string;
    text: string;
    resourceVersion: string;
    revealed: boolean;
  }
  const drafts = new Map<string, Draft>();
</script>

<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import CodeEditor from "./editor/CodeEditor.svelte";
  import DiffView from "./editor/DiffView.svelte";
  import { commands } from "../bindings";
  import type { FieldTakeover } from "../bindings";
  import type { Target } from "../stores/detail.svelte";
  import { confirmDiscard, markSaved, markUnsaved } from "../stores/unsaved.svelte";
  import { errorLine, resourceVersionOf } from "../yamlEdit";
  import type { DiffLayout } from "../editor/setup";

  let { target }: { target: Target } = $props();

  // The panel is remounted for every object, so `target` is fixed here.
  // svelte-ignore state_referenced_locally
  const isSecret = target.resource.group === "" && target.resource.kind === "Secret";
  // svelte-ignore state_referenced_locally
  const id = `yaml|${target.resource.group}/${target.resource.kind}|${target.namespace}/${target.name}`;

  /** What the server holds (as last loaded): the left side of the diff. */
  let base = $state("");
  /** The editor's text. */
  let text = $state("");
  /** `metadata.resourceVersion` of `base`: a save checks it is still current. */
  let resourceVersion = $state("");
  let status = $state("");
  /** Validation or save error from the API server, and the line it names. */
  let error = $state("");
  let errorAt = $state<number | null>(null);
  /** Secret values are hidden until the user asks for them. */
  let revealed = $state(false);
  /** Nothing to save until a load succeeded. */
  let loaded = $state(false);
  let busy = $state(false);
  /** Reviewing the diff before saving. */
  let reviewing = $state(false);
  let layout = $state<DiffLayout>("unified");
  /** Set when the object changed on the server since it was loaded. */
  let conflict = $state(false);
  /** Fields the save would take over (review), or took over (after it). */
  let pendingTakeovers = $state.raw<FieldTakeover[]>([]);
  let takeovers = $state.raw<FieldTakeover[]>([]);

  const locked = $derived(isSecret && !revealed);
  const dirty = $derived(loaded && text !== base);

  $effect(() => {
    if (dirty) markUnsaved(id, discardEdits);
    else markSaved(id);
  });

  /** The user discarded the edits (here, or from the guard dialog). */
  function discardEdits() {
    drafts.delete(id);
    text = base;
    reviewing = false;
  }

  function clearMessages() {
    status = "";
    error = "";
    errorAt = null;
    conflict = false;
    takeovers = [];
    pendingTakeovers = [];
  }

  async function fetchYaml(): Promise<string> {
    return commands.cmdGetResourceYaml(target.resource, target.namespace, target.name, revealed);
  }

  async function load() {
    clearMessages();
    reviewing = false;
    loaded = false;
    status = "Loading…";
    try {
      const yaml = await fetchYaml();
      base = yaml;
      text = yaml;
      resourceVersion = resourceVersionOf(yaml);
      loaded = true;
      status = "";
    } catch (e) {
      base = text = "";
      status = "Could not load YAML: " + e;
    }
  }

  /** Reload, Reveal and Hide drop the edits: ask first. */
  async function reloadAsking(reveal: boolean) {
    if (dirty && !(await confirmDiscard())) return;
    revealed = reveal;
    await load();
  }

  function showError(prefix: string, e: unknown) {
    error = prefix + e;
    errorAt = errorLine(String(e), text);
    reviewing = false;
  }

  function apply(dryRun: boolean) {
    return commands.cmdApplyResourceYaml(
      target.resource,
      target.namespace,
      target.name,
      text,
      resourceVersion,
      dryRun,
    );
  }

  /** Validate with a server dry run, then show the diff. */
  async function review() {
    clearMessages();
    busy = true;
    status = "Validating…";
    try {
      const res = await apply(true);
      status = "";
      if (res.outcome === "conflict") {
        conflict = true;
        return;
      }
      pendingTakeovers = res.takeovers;
      reviewing = true;
    } catch (e) {
      status = "";
      showError("The server rejected this YAML: ", e);
    } finally {
      busy = false;
    }
  }

  async function save() {
    busy = true;
    status = "Saving…";
    try {
      const res = await apply(false);
      if (res.outcome === "conflict") {
        status = "";
        reviewing = false;
        conflict = true;
        return;
      }
      // Reload so the editor shows what the server now holds.
      await load();
      drafts.delete(id);
      takeovers = res.takeovers;
      status = "Saved.";
    } catch (e) {
      status = "";
      showError("Could not save YAML: ", e);
    } finally {
      busy = false;
    }
  }

  /** Conflict: keep the edits, compare them with the server's latest. */
  async function diffLatest() {
    busy = true;
    try {
      const latest = await fetchYaml();
      base = latest;
      resourceVersion = resourceVersionOf(latest);
    } catch (e) {
      busy = false;
      status = "Could not load YAML: " + e;
      return;
    }
    busy = false;
    await review();
  }

  onMount(() => {
    const d = drafts.get(id);
    if (!d) return void load();
    drafts.delete(id);
    ({ base, text, resourceVersion, revealed } = d);
    loaded = true;
  });

  onDestroy(() => {
    // Still registered as unsaved: the guard asks before dropping these.
    if (dirty) drafts.set(id, { base, text, resourceVersion, revealed });
  });
</script>

<div class="section-header">
  <h3>YAML{#if dirty}<span class="dirty" title="Unsaved changes"> ●</span>{/if}</h3>
  {#if reviewing}
    <div class="layout" role="group" aria-label="Diff layout">
      <button type="button" class="btn" aria-pressed={layout === "unified"} onclick={() => (layout = "unified")}
        >Unified</button
      >
      <button type="button" class="btn" aria-pressed={layout === "split"} onclick={() => (layout = "split")}
        >Side by side</button
      >
    </div>
    <button type="button" class="btn" disabled={busy} onclick={() => (reviewing = false)}>Back to editing</button>
    <button type="button" class="btn primary" disabled={busy} onclick={save}>Save</button>
  {:else}
    {#if isSecret}
      {#if revealed}
        <button type="button" class="btn" disabled={busy} onclick={() => reloadAsking(false)}>Hide values</button>
      {:else}
        <button type="button" class="btn" disabled={busy} onclick={() => reloadAsking(true)}>Reveal values</button>
      {/if}
    {/if}
    <button type="button" class="btn" disabled={busy} onclick={() => reloadAsking(revealed)}>Reload</button>
    <button type="button" class="btn primary" onclick={review} disabled={locked || !dirty || busy}>Review</button>
  {/if}
</div>

{#if conflict}
  <div class="banner conflict" role="alert">
    This object changed on the server since you loaded it.
    <div class="banner-actions">
      <button type="button" class="btn" disabled={busy} onclick={load}>Reload (lose my edits)</button>
      <button type="button" class="btn" disabled={busy} onclick={diffLatest}>Diff against the latest</button>
    </div>
  </div>
{/if}
{#if error}
  <div class="banner error" role="alert">
    {error}
    {#if errorAt !== null}<span class="muted"> (line {errorAt})</span>{/if}
  </div>
{/if}

<div class="pane">
  {#if reviewing}
    {#if base === text}
      <p class="muted nochange">No changes.</p>
    {:else}
      <DiffView original={base} modified={text} {layout} label="{target.resource.kind} YAML changes" />
    {/if}
  {:else}
    <CodeEditor
      value={text}
      readonly={locked || !loaded}
      label="{target.resource.kind} YAML"
      markLine={errorAt}
      onChange={(v) => {
        text = v;
        if (errorAt !== null) errorAt = null;
      }}
    />
  {/if}
</div>

<p class="detail-status muted" aria-live="polite">
  {#if reviewing}
    Server-side validation passed. Review the changes, then save.
  {:else}
    {status}{#if locked && !status}Values are hidden. Reveal them to edit this Secret.{/if}
  {/if}
</p>
{#if reviewing && pendingTakeovers.length > 0}
  <div class="takeover" role="note">
    Saving will take over fields another tool is managing. That tool may change them back the next time it
    runs.
    {@render fieldList(pendingTakeovers)}
  </div>
{/if}
{#if takeovers.length > 0}
  <div class="takeover" role="note">
    This save took over fields another tool was managing. That tool may change them back the next time it
    runs.
    {@render fieldList(takeovers)}
  </div>
{/if}

{#snippet fieldList(list: FieldTakeover[])}
  <ul>
    {#each list as t (t.manager)}
      <li>
        from <strong>{t.manager}</strong>:
        {#each t.fields as f, i (f)}<code>{f}</code>{i < t.fields.length - 1 ? ", " : ""}{/each}
      </li>
    {/each}
  </ul>
{/snippet}

<style>
  .pane {
    height: 50vh;
    min-height: 220px;
    border-top: 1px solid var(--border);
    resize: vertical;
    overflow: hidden;
  }
  .dirty {
    color: var(--amber);
    font-size: 10px;
    vertical-align: middle;
  }
  .layout {
    display: flex;
    gap: 4px;
    margin-left: auto;
  }
  .layout .btn[aria-pressed="true"] {
    border-color: var(--accent);
    color: var(--text);
  }
  .nochange {
    padding: 10px 16px;
    margin: 0;
    font-size: 12px;
  }
  .banner {
    margin: 0 16px 8px;
    padding: 8px 10px;
    border-radius: 4px;
    font-size: 12px;
    word-break: break-word;
  }
  .banner.conflict {
    border: 1px solid var(--amber);
  }
  .banner.error {
    border: 1px solid var(--red);
    color: #f5a3a5;
  }
  .banner-actions {
    display: flex;
    gap: 6px;
    margin-top: 6px;
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
