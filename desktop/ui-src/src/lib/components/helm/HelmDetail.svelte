<script lang="ts">
  import { onMount, untrack } from "svelte";
  import CodeEditor from "../editor/CodeEditor.svelte";
  import HelmHistory from "./HelmHistory.svelte";
  import { commands } from "../../bindings";
  import type { HelmRelease, HelmResource } from "../../bindings";
  import { chartLabel, rollbackTargets, statusClass } from "../../helm";
  import { kindOf, openObject } from "../../stores/app.svelte";
  import { helm, requestHelmAction, selectRelease } from "../../stores/helm.svelte";
  import type { HelmSelection } from "../../stores/helm.svelte";
  import { registerTabs } from "../../stores/keyboard.svelte";

  /** Detail panel of one Helm release. Remounted for every release. */
  let { target }: { target: HelmSelection } = $props();

  const tabs = [
    { id: "overview", label: "Overview" },
    { id: "values", label: "Values" },
    { id: "manifest", label: "Manifest" },
    { id: "history", label: "History" },
    { id: "resources", label: "Resources" },
  ];
  let tab = $state("overview");
  /** The revision shown; 0 = the newest. */
  let revision = $state(0);
  let reveal = $state(false);
  let allValues = $state(false);
  let release = $state.raw<HelmRelease | null>(null);
  let loadError = $state("");
  let loading = $state(false);
  let seq = 0;

  async function load() {
    const n = ++seq;
    loading = true;
    try {
      const r = await commands.cmdHelmGet(target.namespace, target.name, revision, reveal);
      if (n !== seq) return;
      release = r;
      loadError = "";
    } catch (e) {
      if (n !== seq) return;
      loadError = String(e);
    } finally {
      if (n === seq) loading = false;
    }
  }

  // Reload on a revision or reveal change, and after a rollback.
  $effect(() => {
    void revision;
    void reveal;
    void helm.nonce;
    untrack(load);
  });

  onMount(() => registerTabs({ count: () => tabs.length, select: (i) => (tab = tabs[i].id) }));

  const newest = $derived(release?.history[0]?.revision ?? 0);
  const isNewest = $derived(!!release && release.revision === newest);
  const canRollback = $derived(!!release && rollbackTargets(release.history, newest).length > 0);
  const helmBlocked = $derived(helm.helmMissing);

  function rollback(to: number) {
    if (!release) return;
    requestHelmAction({
      action: "rollback",
      namespace: release.namespace,
      name: release.name,
      revision: to,
      current: newest,
    });
  }

  function rollbackDefault() {
    if (!release) return;
    // From an older revision: to it. From the newest: to the one before.
    const to = isNewest ? (rollbackTargets(release.history, newest)[0] ?? 0) : release.revision;
    rollback(to);
  }

  function uninstall() {
    if (!release) return;
    requestHelmAction({
      action: "uninstall",
      namespace: release.namespace,
      name: release.name,
      revision: 0,
      current: newest,
    });
  }

  function link(r: HelmResource) {
    const kind = kindOf(r.api_version, r.kind);
    if (!kind) return null;
    return () => openObject(kind, kind.resource.namespaced ? r.namespace : "", r.name);
  }
</script>

<aside data-testid="helm-detail">
  <div class="header">
    <span class="title"><span class="muted">Helm release</span> {target.namespace}/{target.name}</span>
    <button type="button" class="close" aria-label="Close details" onclick={() => selectRelease(null)}>✕</button>
  </div>

  <div class="bar">
    <label class="rev">
      Revision
      <select class="select" aria-label="Revision" bind:value={revision} disabled={!release}>
        <option value={0}>newest{newest ? ` (${newest})` : ""}</option>
        {#each release?.history ?? [] as h (h.revision)}
          {#if h.revision !== newest}<option value={h.revision}>{h.revision} ({h.status})</option>{/if}
        {/each}
      </select>
    </label>
    <button
      type="button"
      class="btn"
      disabled={!canRollback || !!helmBlocked}
      title={helmBlocked || (canRollback ? "Roll back with helm" : "There is no other revision to roll back to")}
      onclick={rollbackDefault}>{isNewest || !release ? "Rollback…" : `Roll back to ${release.revision}…`}</button
    >
    <button
      type="button"
      class="btn danger"
      disabled={!release || !!helmBlocked}
      title={helmBlocked || "Uninstall with helm"}
      onclick={uninstall}>Uninstall…</button
    >
  </div>
  {#if helmBlocked}
    <p class="note muted" data-testid="helm-missing">{helmBlocked}</p>
  {/if}

  <div class="tabs" role="tablist">
    {#each tabs as t (t.id)}
      <button
        type="button"
        role="tab"
        aria-selected={tab === t.id}
        class:active={tab === t.id}
        onclick={() => (tab = t.id)}>{t.label}</button
      >
    {/each}
  </div>

  {#if loadError}
    <p class="error-text pad" role="alert">Could not load the release: {loadError}</p>
  {:else if !release}
    <p class="muted pad">Loading…</p>
  {:else if tab === "overview"}
    <div class="body">
      <dl>
        <dt>Status</dt>
        <dd class={statusClass(release.status)} data-testid="helm-status">{release.status}</dd>
        <dt>Revision</dt>
        <dd>{release.revision}{isNewest ? "" : ` (newest is ${newest})`}</dd>
        <dt>Chart</dt>
        <dd>{chartLabel(release.chart, release.chart_version)}</dd>
        <dt>App version</dt>
        <dd>{release.app_version}</dd>
        <dt>Description</dt>
        <dd>{release.description}</dd>
        <dt>First deployed</dt>
        <dd>{release.first_deployed}</dd>
        <dt>Last deployed</dt>
        <dd>{release.last_deployed}</dd>
        <dt>Storage</dt>
        <dd>{release.storage === "secret" ? "Secrets" : "ConfigMaps"}</dd>
      </dl>
      <h3>Notes</h3>
      {#if release.notes.trim()}
        <pre class="notes" data-testid="helm-notes">{release.notes}</pre>
      {:else}
        <p class="muted small">The chart has no notes.</p>
      {/if}
    </div>
  {:else if tab === "values"}
    {@const text = allValues ? release.computed_values : release.values}
    <div class="toolbar">
      <label class="check"
        ><input type="checkbox" bind:checked={allValues} /> Include chart defaults</label
      >
    </div>
    {#if text}
      <div class="editor"><CodeEditor value={text} readonly label="Values" testid="helm-values" /></div>
    {:else}
      <p class="muted pad">No user-supplied values: the release uses the chart defaults.</p>
    {/if}
  {:else if tab === "manifest"}
    {#if release.secrets_hidden}
      <div class="toolbar">
        <span class="muted small">Secret values are hidden.</span>
        <button type="button" class="btn" onclick={() => (reveal = true)} disabled={loading}>Reveal</button>
      </div>
    {:else if reveal}
      <div class="toolbar">
        <span class="muted small">Secret values are shown.</span>
        <button type="button" class="btn" onclick={() => (reveal = false)}>Hide</button>
      </div>
    {/if}
    <div class="editor"><CodeEditor value={release.manifest} readonly label="Manifest" testid="helm-manifest" /></div>
  {:else if tab === "history"}
    <HelmHistory {release} {reveal} canAct={!helmBlocked} onrollback={rollback} />
  {:else if tab === "resources"}
    <ul class="resources" data-testid="helm-resources">
      {#each release.resources as r, i (i)}
        {@const open = link(r)}
        <li>
          <span class="muted">{r.kind}</span>
          {#if open}
            <button type="button" class="linkish" onclick={open}>{r.name}</button>
          {:else}
            <span title="This kind is not served by the cluster">{r.name}</span>
          {/if}
          {#if r.namespace !== release.namespace}<span class="muted small">({r.namespace})</span>{/if}
        </li>
      {:else}
        <li class="muted">The manifest has no objects.</li>
      {/each}
    </ul>
  {/if}
</aside>

<style>
  aside {
    width: 560px;
    flex-shrink: 0;
    background: var(--bg-raised);
    border-left: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  aside > :global(*) {
    flex-shrink: 0;
  }
  .header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border);
    font-weight: 600;
    font-size: 13px;
    word-break: break-all;
  }
  .close {
    background: transparent;
    color: var(--text-muted);
    border: none;
    font-size: 14px;
    cursor: pointer;
  }
  .bar,
  .toolbar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 16px;
    font-size: 12px;
  }
  .rev {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-right: auto;
  }
  .btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
  .btn.danger {
    border-color: var(--red);
    color: #f5a3a5;
  }
  .note {
    margin: 0;
    padding: 0 16px 8px;
    font-size: 12px;
  }
  .tabs {
    display: flex;
    gap: 4px;
    padding: 0 12px;
    border-bottom: 1px solid var(--border);
  }
  .tabs button {
    background: transparent;
    border: none;
    border-bottom: 2px solid transparent;
    color: var(--text-muted);
    padding: 8px 10px;
    font-size: 13px;
    cursor: pointer;
  }
  .tabs button.active {
    color: var(--text);
    border-bottom-color: var(--accent);
  }
  .pad,
  .body {
    padding: 12px 16px;
    margin: 0;
  }
  .body {
    overflow-y: auto;
    flex: 1;
    flex-shrink: 1;
  }
  dl {
    display: grid;
    grid-template-columns: 120px 1fr;
    gap: 4px 12px;
    margin: 0;
    font-size: 13px;
  }
  dt {
    color: var(--text-muted);
  }
  dd {
    margin: 0;
    word-break: break-all;
  }
  h3 {
    margin: 16px 0 8px;
    font-size: 13px;
  }
  .notes {
    margin: 0;
    padding: 8px 10px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    font-size: 12px;
    white-space: pre-wrap;
  }
  .small {
    font-size: 12px;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .editor {
    flex: 1;
    flex-shrink: 1;
    min-height: 0;
    overflow: auto;
  }
  .resources {
    list-style: none;
    margin: 0;
    padding: 12px 16px;
    font-size: 13px;
    overflow-y: auto;
  }
  .resources li {
    padding: 3px 0;
    display: flex;
    gap: 8px;
    align-items: baseline;
  }
  .linkish {
    background: none;
    border: none;
    padding: 0;
    color: var(--accent);
    cursor: pointer;
    font: inherit;
  }
  .linkish:hover {
    text-decoration: underline;
  }
</style>
