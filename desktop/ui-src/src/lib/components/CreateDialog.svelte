<script lang="ts">
  import { onDestroy } from "svelte";
  import CodeEditor from "./editor/CodeEditor.svelte";
  import { commands } from "../bindings";
  import type { ApplyResult } from "../bindings";
  import { createUi, closeCreate } from "../stores/create.svelte";
  import { kindOf, loadKinds, openObject } from "../stores/app.svelte";
  import { confirmDiscard, markSaved, markUnsaved, unsaved } from "../stores/unsaved.svelte";
  import { isOk, resultLine, resultTitle, statusLabel, summarize } from "../manifests";
  import type { Summary } from "../manifests";

  const OWNER = "create-dialog";

  // Mounted for each opening, so the initial text is fixed here.
  let base = $state(createUi.initial);
  let text = $state(createUi.initial);
  const namespace = createUi.namespace || "default";

  let busy = $state(false);
  let status = $state("");
  let error = $state("");
  let results = $state.raw<ApplyResult[]>([]);
  /** Whether `results` come from a dry run. */
  let resultsDryRun = $state(true);
  let summary = $state<Summary | null>(null);
  let markAt = $state<number | null>(null);
  let dragging = $state(false);
  let fileInput: HTMLInputElement;

  const dirty = $derived(text !== base);
  const failed = $derived(results.filter((r) => r.status === "failed").length);
  /** Some documents failed but others are fine: offer to apply just those. */
  const canApplyValid = $derived(failed > 0 && failed < results.length && !busy);

  $effect(() => {
    if (dirty) markUnsaved(OWNER, () => (text = base));
    else markSaved(OWNER);
  });
  onDestroy(() => markSaved(OWNER));

  async function close() {
    if (busy || unsaved.asking) return;
    if (dirty && !(await confirmDiscard())) return;
    closeCreate();
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape" && !unsaved.asking) void close();
  }

  async function run(dryRun: boolean, skipInvalid = false) {
    busy = true;
    error = "";
    markAt = null;
    status = dryRun ? "Validating…" : "Creating…";
    try {
      const res = await commands.cmdApplyManifests(text, createUi.namespace, dryRun, skipInvalid);
      results = res;
      resultsDryRun = dryRun;
      summary = summarize(res, dryRun);
      const firstFailed = res.find((r) => r.status === "failed");
      if (firstFailed) markAt = resultLine(firstFailed, text);
      if (!dryRun && res.some((r) => isOk(r.status))) {
        // Applied: these edits are used up, and the objects show up in the
        // table through its watch. New kinds (a CRD) need a new discovery.
        base = text;
        if (res.some((r) => isOk(r.status) && (!kindOf(r.api_version, r.kind) || r.kind === "CustomResourceDefinition"))) {
          await loadKinds();
        }
      }
    } catch (e) {
      results = [];
      summary = null;
      error = String(e);
    } finally {
      status = "";
      busy = false;
    }
  }

  function pick(r: ApplyResult) {
    if (!resultsDryRun && isOk(r.status)) {
      const kind = kindOf(r.api_version, r.kind);
      if (kind) {
        closeCreate();
        openObject(kind, r.namespace, r.name);
        return;
      }
    }
    markAt = resultLine(r, text);
  }

  async function load(file: File | undefined) {
    if (!file) return;
    if (dirty && !(await confirmDiscard())) return;
    try {
      text = await file.text();
      results = [];
      summary = null;
      error = "";
      markAt = null;
    } catch (e) {
      error = `Could not read ${file.name}: ${e}`;
    }
  }

  function ondrop(e: DragEvent) {
    e.preventDefault();
    dragging = false;
    void load(e.dataTransfer?.files[0]);
  }

  function ondragover(e: DragEvent) {
    if (!e.dataTransfer?.types.includes("Files")) return;
    e.preventDefault();
    dragging = true;
  }
</script>

<svelte:window {onkeydown} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={(e) => e.target === e.currentTarget && close()}>
  <div
    class="dialog"
    class:dragging
    role="dialog"
    tabindex="-1"
    aria-modal="true"
    aria-label="Create resources"
    data-testid="create-dialog"
    {ondrop}
    {ondragover}
    ondragleave={() => (dragging = false)}
  >
    <div class="head">
      <h2>Create resources{#if dirty}<span class="dirty" title="Unsaved changes"> ●</span>{/if}</h2>
      <span class="muted hint">One or more documents, separated by <code>---</code>. Objects without a namespace go to
        <strong>{namespace}</strong>.</span
      >
      <button type="button" class="btn" disabled={busy} onclick={() => fileInput.click()}>Open file…</button>
      <input
        bind:this={fileInput}
        type="file"
        accept=".yaml,.yml,.json,application/yaml,text/yaml"
        hidden
        data-testid="create-file"
        onchange={(e) => {
          void load(e.currentTarget.files?.[0]);
          e.currentTarget.value = "";
        }}
      />
    </div>

    <div class="pane">
      <CodeEditor
        value={text}
        label="New resources YAML"
        testid="create-editor"
        markLine={markAt}
        onChange={(v) => {
          text = v;
          if (markAt !== null) markAt = null;
        }}
      />
      {#if dragging}<div class="drop">Drop a YAML file to open it</div>{/if}
    </div>

    {#if error}<p class="error-text banner" role="alert">{error}</p>{/if}
    {#if summary}
      <p class="summary" class:ok={summary.ok} class:bad={!summary.ok} role="status" data-testid="create-summary">
        {summary.text}
      </p>
    {/if}
    {#if results.length > 0}
      <ul class="results" aria-label="Results">
        {#each results as r, i (i)}
          <li>
            <button
              type="button"
              class="result {r.status}"
              data-testid="create-result"
              title={!resultsDryRun && isOk(r.status) ? "Open this object" : `Go to line ${resultLine(r, text)}`}
              onclick={() => pick(r)}
            >
              <span class="badge">{statusLabel(r.status, resultsDryRun)}</span>
              <span class="what">{resultTitle(r)}</span>
              <span class="muted line">line {r.line}</span>
              {#if r.message}<span class="message">{r.message}</span>{/if}
              {#if r.takeovers.length > 0}
                <span class="message takeover">
                  {resultsDryRun ? "Would take over" : "Took over"} fields from
                  {r.takeovers.map((t) => `${t.manager} (${t.fields.join(", ")})`).join("; ")}
                </span>
              {/if}
            </button>
          </li>
        {/each}
      </ul>
    {/if}

    <div class="buttons">
      <span class="muted status" aria-live="polite">{status}</span>
      <button type="button" class="btn" disabled={busy} onclick={close}>{dirty ? "Cancel" : "Close"}</button>
      {#if canApplyValid}
        <button type="button" class="btn" onclick={() => run(false, true)}>Create the valid ones only</button>
      {/if}
      <button type="button" class="btn" disabled={busy || !text.trim()} onclick={() => run(true)}>Validate</button>
      <button type="button" class="btn primary" disabled={busy || !text.trim()} onclick={() => run(false)}
        >Create</button
      >
    </div>
  </div>
</div>

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
    width: min(960px, calc(100vw - 32px));
    height: min(760px, calc(100vh - 32px));
    display: flex;
    flex-direction: column;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 8px;
    overflow: hidden;
  }
  .dialog.dragging {
    border-color: var(--accent);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 16px;
  }
  h2 {
    margin: 0;
    font-size: 15px;
    white-space: nowrap;
  }
  .hint {
    flex: 1;
    font-size: 12px;
  }
  .dirty {
    color: var(--amber);
    font-size: 10px;
    vertical-align: middle;
  }
  .pane {
    position: relative;
    flex: 1;
    min-height: 160px;
    border-top: 1px solid var(--border);
    border-bottom: 1px solid var(--border);
    overflow: hidden;
  }
  .drop {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(79, 156, 247, 0.15);
    font-size: 14px;
    pointer-events: none;
  }
  .banner,
  .summary {
    margin: 8px 16px 0;
    font-size: 12px;
  }
  .summary.ok {
    color: var(--green);
  }
  .summary.bad {
    color: var(--red);
  }
  .results {
    list-style: none;
    margin: 6px 0 0;
    padding: 0 16px;
    max-height: 30%;
    overflow: auto;
  }
  .result {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 4px 10px;
    width: 100%;
    padding: 5px 8px;
    border: 0;
    border-radius: 4px;
    background: none;
    color: inherit;
    font: inherit;
    font-size: 12px;
    text-align: left;
    cursor: pointer;
  }
  .result:hover {
    background: var(--bg-hover);
  }
  .badge {
    min-width: 96px;
    font-weight: 600;
  }
  .created .badge,
  .configured .badge {
    color: var(--green);
  }
  .failed .badge {
    color: var(--red);
  }
  .skipped .badge,
  .pending .badge {
    color: var(--amber);
  }
  .what {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
  }
  .line {
    margin-left: auto;
  }
  .message {
    flex-basis: 100%;
    padding-left: 106px;
    word-break: break-word;
  }
  .failed .message {
    color: #f5a3a5;
  }
  .takeover {
    color: var(--amber);
  }
  .buttons {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
    padding: 12px 16px;
  }
  .status {
    margin-right: auto;
    font-size: 12px;
  }
  .btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
</style>
