<script lang="ts">
  import { untrack } from "svelte";
  import DiffView from "../editor/DiffView.svelte";
  import { commands } from "../../bindings";
  import type { HelmRelease } from "../../bindings";
  import { ageOf, defaultDiffPair, statusClass } from "../../helm";

  /** The History tab: the stored revisions and a diff of any two of them. */
  let {
    release,
    reveal,
    canAct,
    onrollback,
  }: {
    release: HelmRelease;
    /** Diff manifests with Secret values shown. */
    reveal: boolean;
    canAct: boolean;
    onrollback: (revision: number) => void;
  } = $props();

  const newest = $derived(release.history[0]?.revision ?? 0);
  // svelte-ignore state_referenced_locally
  const initial = defaultDiffPair(release.history, release.revision);
  let from = $state(initial?.[0] ?? 0);
  let to = $state(initial?.[1] ?? 0);
  let what = $state<"values" | "manifest">("values");
  let texts = $state.raw<[string, string] | null>(null);
  let diffError = $state("");
  const now = Date.now();

  /** Fetched revisions, by `revision|reveal`. */
  const cache = new Map<string, Promise<HelmRelease>>();
  function fetchRevision(rev: number): Promise<HelmRelease> {
    if (rev === release.revision && reveal === !release.secrets_hidden) return Promise.resolve(release);
    const key = `${rev}|${reveal}`;
    let p = cache.get(key);
    if (!p) {
      p = commands.cmdHelmGet(release.namespace, release.name, rev, reveal);
      p.catch(() => cache.delete(key));
      cache.set(key, p);
    }
    return p;
  }

  let seq = 0;
  $effect(() => {
    const [a, b, w] = [from, to, what];
    void reveal;
    if (!a || !b) return;
    untrack(async () => {
      const n = ++seq;
      try {
        const [ra, rb] = await Promise.all([fetchRevision(a), fetchRevision(b)]);
        if (n !== seq) return;
        texts = w === "values" ? [ra.values, rb.values] : [ra.manifest, rb.manifest];
        diffError = "";
      } catch (e) {
        if (n === seq) diffError = String(e);
      }
    });
  });
</script>

<div class="history" data-testid="helm-history">
  <table>
    <thead>
      <tr><th>Revision</th><th>Status</th><th>Updated</th><th></th></tr>
    </thead>
    <tbody>
      {#each release.history as h (h.revision)}
        <tr data-testid="helm-revision">
          <td>{h.revision}</td>
          <td class={statusClass(h.status)}>{h.status}</td>
          <td>{ageOf(h.updated, now)}</td>
          <td class="right">
            {#if h.revision !== newest}
              <button type="button" class="btn" disabled={!canAct} onclick={() => onrollback(h.revision)}
                >Roll back to {h.revision}…</button
              >
            {:else}
              <span class="muted small">current</span>
            {/if}
          </td>
        </tr>
      {/each}
    </tbody>
  </table>

  {#if release.history.length > 1}
    <div class="diff-bar">
      <span>Diff</span>
      <select class="select" aria-label="Diff from revision" bind:value={from}>
        {#each release.history as h (h.revision)}<option value={h.revision}>{h.revision}</option>{/each}
      </select>
      <span>→</span>
      <select class="select" aria-label="Diff to revision" bind:value={to}>
        {#each release.history as h (h.revision)}<option value={h.revision}>{h.revision}</option>{/each}
      </select>
      <div class="seg" role="group" aria-label="What to compare">
        <button type="button" class="btn" class:active={what === "values"} onclick={() => (what = "values")}
          >Values</button
        >
        <button type="button" class="btn" class:active={what === "manifest"} onclick={() => (what = "manifest")}
          >Manifest</button
        >
      </div>
    </div>
    {#if diffError}
      <p class="error-text pad" role="alert">Could not load the revisions: {diffError}</p>
    {:else if texts}
      {#if texts[0] === texts[1]}
        <p class="muted pad" data-testid="helm-diff-same">No differences.</p>
      {:else}
        <div class="diff" data-testid="helm-diff">
          <DiffView original={texts[0]} modified={texts[1]} label="Revision {from} to {to}" />
        </div>
      {/if}
    {/if}
  {/if}
</div>

<style>
  .history {
    flex: 1;
    flex-shrink: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow-y: auto;
  }
  table {
    font-size: 13px;
  }
  th,
  td {
    padding: 6px 16px;
  }
  .right {
    text-align: right;
  }
  .small {
    font-size: 12px;
  }
  .btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
  .diff-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 16px;
    font-size: 12px;
    border-bottom: 1px solid var(--border);
  }
  .seg {
    display: flex;
    gap: 4px;
    margin-left: auto;
  }
  .pad {
    margin: 0;
    padding: 10px 16px;
    font-size: 12px;
  }
  .diff {
    flex: 1;
    min-height: 240px;
  }
</style>
