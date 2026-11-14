<script lang="ts">
  import { onMount } from "svelte";
  import YamlEditor from "./YamlEditor.svelte";
  import LogViewer from "./LogViewer.svelte";
  import { WORKLOAD_KINDS } from "../logs";
  import { commands } from "../bindings";
  import type { OwnerLink, ResourceDetail } from "../bindings";
  import { kindOf, openObject } from "../stores/app.svelte";
  import { closeDetail } from "../stores/detail.svelte";
  import type { Target } from "../stores/detail.svelte";

  /** Detail panel for any kind without a dedicated one (pods have their own). */
  let { target }: { target: Target } = $props();

  let info = $state.raw<ResourceDetail | null>(null);
  let loadError = $state("");

  /** Workloads get a Logs tab with their pods' logs merged. */
  const isWorkload = $derived(
    (target.resource.group === "apps" || target.resource.group === "batch") &&
      WORKLOAD_KINDS.includes(target.resource.kind),
  );
  const workload = $derived({ kind: target.resource.kind, namespace: target.namespace, name: target.name });
  const tabs = $derived([
    { id: "overview", label: "Overview" },
    ...(isWorkload ? [{ id: "logs", label: "Logs" }] : []),
    { id: "yaml", label: "YAML" },
  ]);
  let tab = $state("overview");

  let confirmDelete = $state(false);
  let deleting = $state(false);
  let deleteError = $state("");

  async function load() {
    try {
      info = await commands.cmdGetResourceDetail(target.resource, target.namespace, target.name);
      loadError = "";
    } catch (e) {
      loadError = String(e);
    }
  }

  onMount(load);

  async function remove() {
    deleting = true;
    deleteError = "";
    try {
      await commands.cmdDeleteResource(target.resource, target.namespace, target.name);
      closeDetail();
    } catch (e) {
      deleteError = "Could not delete: " + e;
      deleting = false;
      confirmDelete = false;
    }
  }

  /** The kind an owner reference points at, if discovery knows it. */
  function ownerKind(o: OwnerLink) {
    return kindOf(o.api_version, o.kind);
  }

  function openOwner(o: OwnerLink) {
    const kind = ownerKind(o);
    if (!kind) return;
    // Owners live in the object's namespace, or are cluster scoped.
    openObject(kind, kind.resource.namespaced ? target.namespace : "", o.name);
  }

  const title = $derived(target.namespace ? `${target.namespace}/${target.name}` : target.name);
</script>

<aside>
  <div class="header">
    <span class="title"><span class="muted">{target.resource.kind}</span> {title}</span>
    <button type="button" class="close" aria-label="Close details" onclick={closeDetail}>✕</button>
  </div>

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

  {#if tab === "overview"}
    <div class="body">
      {#if loadError}
        <p class="error-text">Could not load details: {loadError}</p>
      {:else if !info}
        <p class="muted">Loading…</p>
      {:else}
        <dl class="meta">
          <dt>Kind</dt>
          <dd>{info.kind} <span class="muted">({info.api_version})</span></dd>
          <dt>Name</dt>
          <dd>{info.name}</dd>
          {#if info.namespace}
            <dt>Namespace</dt>
            <dd>{info.namespace}</dd>
          {/if}
          <dt>Age</dt>
          <dd>{info.age || "—"}</dd>
          <dt>UID</dt>
          <dd class="mono">{info.uid}</dd>
        </dl>

        {#if info.owners.length > 0}
          <h3>Owned by</h3>
          <ul class="plain">
            {#each info.owners as o (o.kind + "/" + o.name)}
              <li>
                {#if ownerKind(o)}
                  <button type="button" class="link" onclick={() => openOwner(o)}>
                    {o.kind}/{o.name}
                  </button>
                {:else}
                  {o.kind}/{o.name}
                {/if}
                {#if o.controller}<span class="muted"> (controller)</span>{/if}
              </li>
            {/each}
          </ul>
        {/if}

        {#if Object.keys(info.labels).length > 0}
          <h3>Labels</h3>
          <div class="chips">
            {#each Object.entries(info.labels) as [k, v] (k)}
              <span class="chip">{k}={v}</span>
            {/each}
          </div>
        {/if}

        {#if Object.keys(info.annotations).length > 0}
          <details>
            <summary>Annotations ({Object.keys(info.annotations).length})</summary>
            <dl class="annotations">
              {#each Object.entries(info.annotations) as [k, v] (k)}
                <dt>{k}</dt>
                <dd class="mono">{v}</dd>
              {/each}
            </dl>
          </details>
        {/if}

        {#if info.finalizers.length > 0}
          <h3>Finalizers</h3>
          <div class="chips">
            {#each info.finalizers as f (f)}<span class="chip">{f}</span>{/each}
          </div>
        {/if}

        {#if info.secret_keys.length > 0}
          <h3>Data</h3>
          <p class="muted small">Values are hidden. Reveal them in the YAML tab.</p>
          <ul class="plain">
            {#each info.secret_keys as k (k.key)}
              <li class="mono">{k.key} <span class="muted">({k.bytes} bytes)</span></li>
            {/each}
          </ul>
        {/if}

        {#if info.conditions.length > 0}
          <h3>Conditions</h3>
          <div class="scroll-x">
            <table>
              <thead><tr><th>Type</th><th>Status</th><th>Reason</th><th>Since</th></tr></thead>
              <tbody>
                {#each info.conditions as c (c.type)}
                  <tr title={c.message}>
                    <td>{c.type}</td>
                    <td class:ok={c.status === "True"} class:bad={c.status === "False"}>{c.status}</td>
                    <td>{c.reason}</td>
                    <td>{c.age}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}

        <h3>Events</h3>
        {#if info.events.length === 0}
          <p class="muted small">No events.</p>
        {:else}
          <div class="scroll-x">
            <table>
              <thead><tr><th>Type</th><th>Reason</th><th>Message</th><th>Age</th></tr></thead>
              <tbody>
                {#each info.events as e, i (i)}
                  <tr>
                    <td class:warn={e.type === "Warning"}>{e.type}</td>
                    <td>{e.reason}{e.count > 1 ? ` ×${e.count}` : ""}</td>
                    <td class="wrap">{e.message}</td>
                    <td>{e.age}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      {/if}
    </div>

    <div class="section-header">
      <h3>Delete</h3>
      {#if confirmDelete}
        <button type="button" class="btn" disabled={deleting} onclick={() => (confirmDelete = false)}>
          Cancel
        </button>
        <button type="button" class="btn danger" disabled={deleting} onclick={remove}>
          {deleting ? "Deleting…" : "Confirm delete"}
        </button>
      {:else}
        <button type="button" class="btn danger" onclick={() => (confirmDelete = true)}>
          Delete {target.resource.kind}
        </button>
      {/if}
    </div>
    {#if deleteError}<p class="detail-status error-text">{deleteError}</p>{/if}
  {:else if tab === "logs" && isWorkload}
    <LogViewer {workload} />
  {:else if tab === "yaml"}
    <YamlEditor {target} />
  {/if}
</aside>

<style>
  aside {
    width: 420px;
    flex-shrink: 0;
    background: var(--bg-raised);
    border-left: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow-y: auto;
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
    flex-shrink: 0;
  }
  .close:hover {
    color: var(--text);
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
  .body {
    padding: 12px 16px;
  }
  .body h3 {
    margin: 14px 0 8px;
    font-size: 13px;
  }
  .body table {
    font-size: 12px;
  }
  .body td.wrap {
    white-space: normal;
    word-break: break-word;
  }
  .scroll-x {
    overflow-x: auto;
  }
  dl.meta {
    display: grid;
    grid-template-columns: 80px 1fr;
    gap: 4px 12px;
    margin: 0;
    font-size: 13px;
  }
  dl.meta dt {
    color: var(--text-muted);
  }
  dl.meta dd {
    margin: 0;
    word-break: break-all;
  }
  .mono {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 12px;
  }
  .small {
    font-size: 12px;
    margin: 0 0 6px;
  }
  ul.plain {
    list-style: none;
    margin: 0;
    padding: 0;
    font-size: 13px;
  }
  ul.plain li {
    padding: 2px 0;
  }
  .link {
    background: transparent;
    border: none;
    color: var(--accent);
    padding: 0;
    font: inherit;
    cursor: pointer;
  }
  .link:hover {
    text-decoration: underline;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .chip {
    background: var(--bg-hover);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 2px 6px;
    font-size: 12px;
    word-break: break-all;
  }
  details {
    margin-top: 12px;
    font-size: 13px;
  }
  summary {
    cursor: pointer;
    font-weight: 600;
  }
  dl.annotations {
    margin: 8px 0 0;
    font-size: 12px;
  }
  dl.annotations dt {
    color: var(--text-muted);
    margin-top: 6px;
    word-break: break-all;
  }
  dl.annotations dd {
    margin: 0;
    word-break: break-all;
    max-height: 6em;
    overflow: auto;
  }
  .ok {
    color: var(--green);
  }
  .bad,
  .warn {
    color: var(--amber);
  }
  .btn.danger {
    border-color: var(--red);
    color: #f5a3a5;
  }
  .btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
</style>
