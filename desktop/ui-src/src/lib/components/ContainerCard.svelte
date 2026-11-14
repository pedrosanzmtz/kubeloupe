<script lang="ts">
  import type { ContainerDetail } from "../bindings";
  import { envSourceText, resourcesText, stateSince, stateText, stateTone } from "../containers";

  /** One container of the pod panel: state, last termination, resources,
   *  ports (each with a Forward button), probes, mounts and env names. */
  let {
    container: c,
    onForward,
    disabled = false,
  }: { container: ContainerDetail; onForward: (port: number) => void; disabled?: boolean } = $props();

  const resources = $derived(resourcesText(c));
</script>

<section class="card" aria-label="Container {c.name}">
  <div class="head">
    <span class="name">{c.name}</span>
    {#if c.kind !== "regular"}<span class="tag">{c.kind}</span>{/if}
    <span class="ready" class:ok={c.ready}>{c.ready ? "ready" : "not ready"}</span>
  </div>
  <div class="image mono" title={c.image_id}>{c.image}</div>

  <div class="state tone-{stateTone(c.state)}" data-testid="container-state">
    {stateText(c.state)}
    {#if stateSince(c.state)}<span class="muted"> · {stateSince(c.state)}</span>{/if}
  </div>
  {#if c.state.message}<div class="message">{c.state.message}</div>{/if}

  <dl>
    <dt>Restarts</dt>
    <dd>{c.restarts}</dd>
    {#if c.last_state.state}
      <dt>Last state</dt>
      <dd class="tone-{stateTone(c.last_state)}" data-testid="last-state">
        {stateText(c.last_state)}
        {#if stateSince(c.last_state)}<span class="muted"> · {stateSince(c.last_state)}</span>{/if}
      </dd>
    {/if}
    {#if resources}
      <dt>Resources</dt>
      <dd>{resources} <span class="muted">(request / limit)</span></dd>
    {/if}
  </dl>

  {#if c.ports.length > 0}
    <h4>Ports</h4>
    <ul>
      {#each c.ports as p (p.port + p.protocol)}
        <li>
          <span class="mono">{p.port}/{p.protocol}</span>
          {#if p.name}<span class="muted">{p.name}</span>{/if}
          {#if p.protocol === "TCP"}
            <button
              type="button"
              class="btn small"
              {disabled}
              aria-label="Forward port {p.port}"
              onclick={() => onForward(p.port)}>Forward</button
            >
          {/if}
        </li>
      {/each}
    </ul>
  {/if}

  {#if c.probes.length > 0}
    <h4>Probes</h4>
    <ul>
      {#each c.probes as p (p.kind)}
        <li><span class="muted">{p.kind}</span> <span class="mono">{p.summary}</span></li>
      {/each}
    </ul>
  {/if}

  {#if c.mounts.length > 0}
    <details>
      <summary>Mounts ({c.mounts.length})</summary>
      <ul>
        {#each c.mounts as m (m.path)}
          <li>
            <span class="mono">{m.path}</span>
            <span class="muted">from {m.name}{m.sub_path ? `/${m.sub_path}` : ""}{m.read_only ? " (ro)" : ""}</span>
          </li>
        {/each}
      </ul>
    </details>
  {/if}

  {#if c.env.length > 0}
    <details>
      <summary>Environment ({c.env.length})</summary>
      <ul>
        {#each c.env as e, i (i)}
          <li><span class="mono">{e.name}</span> <span class="muted">{envSourceText(e.source)}</span></li>
        {/each}
      </ul>
    </details>
  {/if}
</section>

<style>
  .card {
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 8px 10px;
    margin-bottom: 8px;
    font-size: 12px;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .name {
    font-weight: 600;
    font-size: 13px;
  }
  .tag {
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0 4px;
    color: var(--text-muted);
  }
  .ready {
    margin-left: auto;
    color: var(--amber);
  }
  .ready.ok {
    color: var(--green);
  }
  .image {
    color: var(--text-muted);
    word-break: break-all;
    margin: 2px 0 6px;
  }
  .state {
    font-weight: 600;
  }
  .message {
    margin-top: 2px;
    word-break: break-word;
    color: var(--text-muted);
  }
  .tone-ok {
    color: var(--green);
  }
  .tone-warn {
    color: var(--amber);
  }
  .tone-bad {
    color: var(--red);
  }
  dl {
    display: grid;
    grid-template-columns: 80px 1fr;
    gap: 2px 10px;
    margin: 6px 0 0;
  }
  dt {
    color: var(--text-muted);
  }
  dd {
    margin: 0;
    word-break: break-word;
  }
  h4 {
    margin: 8px 0 2px;
    font-size: 12px;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  li {
    padding: 1px 0;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    word-break: break-all;
  }
  details {
    margin-top: 6px;
  }
  summary {
    cursor: pointer;
    font-weight: 600;
  }
  .mono {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
  }
  .btn.small {
    padding: 1px 8px;
    font-size: 11px;
  }
</style>
