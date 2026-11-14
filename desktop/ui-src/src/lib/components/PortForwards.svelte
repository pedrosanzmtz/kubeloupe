<script lang="ts">
  import { onDestroy } from "svelte";
  import { forwardLabel, forwardState, portsLabel } from "../forwards";
  import { app } from "../stores/app.svelte";
  import { copyForwardUrl, forwards, openForward, pollForwards, stopForward } from "../stores/forwards.svelte";

  /** Network → Port Forwards: every forward the backend runs, with its
   *  live status. Forwards outlive this view; leaving it stops nothing. */
  const stopPolling = pollForwards();
  onDestroy(stopPolling);

  $effect(() => {
    const n = forwards.list.length;
    app.count = n === 1 ? "1 forward" : `${n} forwards`;
  });
</script>

<div class="forwards" data-testid="port-forwards">
  <p class="note muted">
    Forwards keep running when you close panels or leave this view. Switching context stops all of them.
  </p>
  {#if forwards.list.length === 0}
    <p class="empty muted">
      No port-forwards. Start one from a pod, Service or workload: right-click it and pick Port forward…, or use the
      Forward buttons in its details.
    </p>
  {:else}
    <table>
      <thead>
        <tr>
          <th>Target</th>
          <th>Ports</th>
          <th>Local URL</th>
          <th>Pod</th>
          <th>Status</th>
          <th><span class="hidden">Actions</span></th>
        </tr>
      </thead>
      <tbody>
        {#each forwards.list as f (f.id)}
          {@const st = forwardState(f)}
          <tr data-testid="forward-row">
            <td class="mono">{forwardLabel(f.target)}</td>
            <td class="mono">{portsLabel(f)}</td>
            <td class="mono">{f.url}</td>
            <td class="mono">{f.status.pod || "—"}</td>
            <td class:error-text={st.tone === "error"} class:ok={st.tone === "ok"}>{st.text}</td>
            <td class="actions">
              <button type="button" class="btn" aria-label="Open {f.url} in the browser" onclick={() => openForward(f)}
                >Open</button
              >
              <button type="button" class="btn" aria-label="Copy {f.url}" onclick={() => copyForwardUrl(f)}>Copy URL</button>
              <button type="button" class="btn" aria-label="Stop {f.url}" onclick={() => stopForward(f.id)}>Stop</button>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>

<style>
  .forwards {
    flex: 1;
    overflow: auto;
    padding: 12px 16px;
  }
  .note,
  .empty {
    font-size: 13px;
    margin: 0 0 12px;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 13px;
  }
  th {
    text-align: left;
    font-weight: 600;
    font-size: 12px;
    color: var(--text-muted);
    padding: 6px 8px;
    border-bottom: 1px solid var(--border);
  }
  td {
    padding: 6px 8px;
    border-bottom: 1px solid var(--border);
    word-break: break-all;
  }
  .mono {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 12px;
  }
  .ok {
    color: var(--green);
  }
  .actions {
    white-space: nowrap;
    word-break: normal;
    text-align: right;
  }
  .actions .btn + .btn {
    margin-left: 4px;
  }
</style>
