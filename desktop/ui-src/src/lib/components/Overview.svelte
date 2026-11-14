<script module lang="ts">
  /** Feed ids, unique across mounts so a stale feed's events are ignored. */
  let nextFeed = 0;
</script>

<script lang="ts">
  import { onDestroy, untrack } from "svelte";
  import { commands, events } from "../bindings";
  import type { EventInfo, Overview_Serialize, OverviewStateChanged } from "../bindings";
  import { app, kinds, openObject, setStatus, showError, stamp } from "../stores/app.svelte";
  import { clusterError } from "../clusterError";
  import { formatAge } from "../format";
  import { capacityBars, eventKind, overviewCount, podCards } from "../overview";

  /** Retry a failed start after this long. */
  const RETRY_MS = 5_000;
  /** Ages are recomputed in the UI; nothing is re-requested. */
  const AGE_TICK_MS = 15_000;

  let overview = $state.raw<Overview_Serialize | null>(null);
  let now = $state(Date.now());
  /** The running feed, or 0. Events of other ids are from stopped feeds. */
  let feed = 0;
  let retry: ReturnType<typeof setTimeout> | null = null;

  // The backend pushes a new summary when something changes (watches, not
  // polling), and a state when the connection changes.
  const listening = Promise.all([
    events.overviewUpdate.listen((e) => {
      if (e.payload.id !== feed) return;
      overview = e.payload.overview;
      app.count = overviewCount(overview.pods, overview.nodes);
      stamp();
    }),
    events.overviewState.listen((e) => {
      if (e.payload.id === feed) onState(e.payload);
    }),
  ]);

  /** The banner message this view set, so it clears only its own. */
  let shown = "";

  function setError(message: string) {
    shown = message;
    showError(message);
  }

  function clearError() {
    if (shown && app.error === shown) showError("");
    shown = "";
  }

  function onState(s: OverviewStateChanged) {
    if (s.state === "live") {
      setStatus(true, "Live");
      clearError();
    } else if (s.state === "reconnecting") {
      const err = clusterError(s.error);
      setStatus(false, "Reconnecting…");
      setError("Overview interrupted, retrying: " + err.message);
    } else {
      setStatus(null, "Connecting…");
    }
  }

  function stop() {
    if (retry) clearTimeout(retry);
    retry = null;
    const id = feed;
    feed = 0;
    if (id) commands.cmdOverviewStop(id).catch(() => {});
  }

  async function start(namespace: string) {
    stop();
    const id = ++nextFeed;
    feed = id;
    setStatus(null, "Connecting…");
    clearError();
    // Listen first: the backend may send the first summary right away.
    await listening;
    if (feed !== id) return;
    try {
      // Parts the user may not see cluster-wide fall back to this namespace.
      await commands.cmdOverviewStart(id, namespace || null);
    } catch (e) {
      if (feed !== id) return;
      const err = clusterError(e);
      setStatus(false, err.status);
      setError(err.message);
      overview = null;
      retry = setTimeout(() => start(app.namespace), RETRY_MS);
    }
  }

  // Start on mount, and again on Refresh or a context switch (both bump the
  // nonce) or when the start-up namespace arrives. The namespace only
  // matters to users who cannot see the whole cluster.
  $effect(() => {
    void app.refreshNonce;
    const namespace = app.namespace;
    untrack(() => start(namespace));
  });

  const ageTimer = setInterval(() => (now = Date.now()), AGE_TICK_MS);

  onDestroy(() => {
    clearInterval(ageTimer);
    stop();
    listening.then((uns) => uns.forEach((un) => un()));
  });

  const podKind = $derived(kinds.all.find((k) => k.id === "v1/pods"));

  function openEvent(e: EventInfo) {
    const kind = eventKind(e, kinds.all);
    if (kind) openObject(kind, kind.resource.namespaced ? e.object_namespace : "", e.object_name);
  }
</script>

<div class="overview">
  {#if overview}
    {@const pods = overview.pods}
    {#if overview.notes.length > 0}
      <ul class="notes muted">
        {#each overview.notes as note (note)}
          <li>{note}</li>
        {/each}
      </ul>
    {/if}
    <div class="cards">
      {#if overview.namespaces !== null}
        <div class="card">
          <div class="num">{overview.namespaces}</div>
          <div class="label">Namespaces</div>
        </div>
      {/if}
      {#if overview.nodes}
        <div class="card" class:alert={overview.nodes.ready < overview.nodes.total}>
          <div class="num">{overview.nodes.ready}/{overview.nodes.total}</div>
          <div class="label">Nodes ready</div>
        </div>
      {/if}
      {#if pods}
        {@const scope = pods.namespace ? ` in ${pods.namespace}` : ""}
        <div class="card">
          <div class="num">{pods.total}</div>
          <div class="label">Pods{scope}</div>
        </div>
        {#each podCards(pods) as c (c.label)}
          <div class="card" class:alert={c.alert} title={c.title}>
            <div class="num">{c.value}</div>
            <div class="label">{c.label}</div>
          </div>
        {/each}
      {/if}
    </div>

    {#if overview.capacity}
      <section class="capacity" aria-label="Capacity">
        <h3>Capacity</h3>
        {#each capacityBars(overview.capacity) as bar (bar.label)}
          <div class="bar-row">
            <span class="bar-label">{bar.label}</span>
            <div class="bar" role="meter" aria-label={bar.label} aria-valuenow={bar.percent} aria-valuemin={0} aria-valuemax={100}>
              <div class="fill" class:high={bar.high} style:width="{bar.percent}%"></div>
            </div>
            <span class="bar-text">{bar.text}</span>
            <span class="bar-usage muted">{bar.usage}</span>
          </div>
        {/each}
        {#if !overview.capacity.metrics}
          <p class="muted small">No usage: the metrics API (metrics-server) is not available.</p>
        {/if}
      </section>
    {/if}

    <div class="sections">
      <section aria-label="Problem pods">
        <h3>
          Problem pods{pods && pods.problems_total > pods.problems.length
            ? ` (${pods.problems.length} of ${pods.problems_total})`
            : ""}
        </h3>
        {#if pods}
          {#each pods.problems as p (p.namespace + "/" + p.name)}
            <div class="problem">
              {#if podKind}
                <button type="button" class="link" onclick={() => podKind && openObject(podKind, p.namespace, p.name)}>
                  {p.namespace}/{p.name}
                </button>
              {:else}
                <span>{p.namespace}/{p.name}</span>
              {/if}
              <span class="reason" class:error={p.reason === p.status}>{p.reason}</span>
              {#if p.restarts > 0}<span class="muted">{p.restarts} restarts</span>{/if}
            </div>
          {:else}
            <p class="muted">None.</p>
          {/each}
        {:else}
          <p class="muted">Not allowed.</p>
        {/if}

        <h3 class="spaced">Pods by namespace</h3>
        {#if pods}
          <table>
            <thead><tr><th>Namespace</th><th>Pods</th></tr></thead>
            <tbody>
              {#each pods.by_namespace as [ns, n] (ns)}
                <tr><td>{ns}</td><td>{n}</td></tr>
              {/each}
            </tbody>
          </table>
        {:else}
          <p class="muted">Not allowed.</p>
        {/if}
      </section>
      <section aria-label="Warnings">
        <h3>
          Warnings{overview.events?.namespace ? ` in ${overview.events.namespace}` : ""}
        </h3>
        {#each overview.events?.items ?? [] as e (e.key)}
          {@const linked = eventKind(e, kinds.all)}
          <div class="event">
            <span class="age">{formatAge(e.time, now, "")}</span>
            <span class="reason">{e.reason}</span>
            {#if linked}
              <button type="button" class="link object" onclick={() => openEvent(e)}>{e.object}</button>
            {:else}
              <span class="object muted">{e.object}</span>
            {/if}
            <span class="message">{e.message}{e.count > 1 ? ` (x${e.count})` : ""}</span>
          </div>
        {:else}
          <p class="muted">{overview.events ? "No warnings." : "Not allowed."}</p>
        {/each}
      </section>
    </div>
  {/if}
</div>

<style>
  .overview {
    flex: 1;
    overflow: auto;
    padding: 16px;
  }
  .cards {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(120px, 1fr));
    gap: 12px;
    margin-bottom: 20px;
  }
  .notes {
    margin: 0 0 16px;
    padding-left: 18px;
    font-size: 12px;
  }
  .card {
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 14px 16px;
  }
  .card.alert {
    border-color: var(--red);
  }
  .card.alert .num {
    color: var(--red);
  }
  .num {
    font-size: 24px;
    font-weight: 700;
  }
  .label {
    color: var(--text-muted);
    font-size: 12px;
    margin-top: 2px;
  }
  .capacity {
    margin-bottom: 20px;
  }
  .bar-row {
    display: grid;
    grid-template-columns: 70px minmax(120px, 1fr) auto auto;
    gap: 12px;
    align-items: center;
    font-size: 13px;
    padding: 4px 0;
  }
  .bar {
    height: 8px;
    background: var(--bg-hover);
    border-radius: 4px;
    overflow: hidden;
  }
  .fill {
    height: 100%;
    background: var(--accent);
  }
  .fill.high {
    background: var(--amber);
  }
  .small {
    font-size: 12px;
  }
  .sections {
    display: grid;
    grid-template-columns: 1fr 2fr;
    gap: 24px;
  }
  h3 {
    margin: 0 0 10px;
    font-size: 14px;
  }
  h3.spaced {
    margin-top: 20px;
  }
  .problem {
    display: flex;
    gap: 10px;
    padding: 6px 0;
    border-bottom: 1px solid var(--border);
    font-size: 13px;
    align-items: baseline;
  }
  .problem .reason.error {
    color: var(--red);
  }
  .event {
    display: flex;
    gap: 12px;
    padding: 8px 10px;
    border-bottom: 1px solid var(--border);
    border-left: 3px solid var(--amber);
    font-size: 13px;
    align-items: baseline;
  }
  .age {
    color: var(--text-muted);
    min-width: 32px;
  }
  .reason {
    font-weight: 600;
    min-width: 110px;
  }
  .object {
    min-width: 180px;
    text-align: left;
  }
  .message {
    flex: 1;
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
</style>
