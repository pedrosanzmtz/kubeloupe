<script lang="ts">
  import { onDestroy, tick } from "svelte";
  import ResourceTable from "../lib/components/ResourceTable.svelte";
  import { EMPTY_METRICS, podKey } from "../lib/columns";
  import type { Metrics, Row } from "../lib/columns";
  import type { PodInfo, PodUsage } from "../lib/bindings";
  import { KeyedStore } from "../lib/stores/keyed.svelte";

  // Same store, table and virtual list as the real Pods view, fed by a
  // synthetic watch: no Tauri, no cluster. See the README for how to run it.
  const store = new KeyedStore<Row>((row) => podKey(row as PodInfo));

  let total = $state(10_000);
  let changesPerBatch = $state(200);
  let batchMs = $state(200);
  let search = $state("");
  let metrics = $state.raw<Metrics>(EMPTY_METRICS);
  let now = $state.raw(Date.now());
  let scrollEl = $state<HTMLDivElement | null>(null);

  let churning = $state(false);
  let scrolling = $state(false);
  let shown = $state(0);
  let totalRows = $state(0);
  let log = $state<string[]>([]);

  // Rolling stats.
  let batchTimes: number[] = [];
  let frameTimes: number[] = [];
  let stats = $state({ fps: 0, p95Frame: 0, maxFrame: 0, longFrames: 0, batchMean: 0, batchP95: 0, batchMax: 0 });

  const PHASES = ["Running", "Running", "Running", "Pending", "Failed", "Succeeded"];
  let serial = 0;
  const nsOf = (i: number) => `ns-${i % 40}`;

  function pod(i: number, phase = PHASES[i % PHASES.length]): PodInfo {
    return {
      created: Math.floor(Date.now() / 1000) - (i % 5000) * 60,
      name: `app-${i}-${(i * 2654435761).toString(36).slice(0, 5)}`,
      namespace: nsOf(i),
      phase,
      node: `node-${i % 50}`,
      age: "",
    };
  }

  function say(msg: string) {
    log = [`${new Date().toLocaleTimeString()}  ${msg}`, ...log].slice(0, 12);
  }

  async function timedBatch(batch: { snapshot: boolean; applied: Row[]; deleted: string[] }) {
    const t0 = performance.now();
    store.applyBatch(batch);
    await tick(); // derived filter/sort + keyed render flush
    return performance.now() - t0;
  }

  async function fill() {
    serial = total;
    const applied = Array.from({ length: total }, (_, i) => pod(i));
    const ms = await timedBatch({ snapshot: true, applied, deleted: [] });
    say(`snapshot of ${total} rows applied + rendered in ${ms.toFixed(1)} ms`);
    refreshMetrics();
  }

  function refreshMetrics() {
    const t0 = performance.now();
    const pods = new Map<string, PodUsage>();
    for (const e of store.entries) {
      const p = e.row as PodInfo;
      pods.set(podKey(p), {
        namespace: p.namespace,
        name: p.name,
        cpu_millicores: Math.floor(Math.random() * 2000),
        memory_bytes: Math.floor(Math.random() * 2 ** 30),
      });
    }
    metrics = { pods, nodes: new Map() };
    now = Date.now();
    say(`metrics merge (${pods.size} pods) built in ${(performance.now() - t0).toFixed(1)} ms`);
  }

  function churnOnce() {
    const entries = store.entries;
    if (entries.length === 0) return;
    const applied: Row[] = [];
    const deleted: string[] = [];
    const updates = Math.floor(changesPerBatch * 0.8);
    const adds = Math.floor(changesPerBatch * 0.1);
    for (let k = 0; k < updates; k++) {
      const e = entries[Math.floor(Math.random() * entries.length)];
      const p = e.row as PodInfo;
      applied.push({ ...p, phase: PHASES[Math.floor(Math.random() * PHASES.length)] });
    }
    for (let k = 0; k < adds; k++) applied.push(pod(serial++));
    for (let k = 0; k < adds; k++) {
      deleted.push(entries[Math.floor(Math.random() * entries.length)].key);
    }
    timedBatch({ snapshot: false, applied, deleted }).then((ms) => batchTimes.push(ms));
  }

  let churnTimer: ReturnType<typeof setInterval> | undefined;
  let metricsTimer: ReturnType<typeof setInterval> | undefined;
  function toggleChurn() {
    churning = !churning;
    clearInterval(churnTimer);
    clearInterval(metricsTimer);
    if (churning) {
      churnTimer = setInterval(churnOnce, batchMs);
      metricsTimer = setInterval(refreshMetrics, 20_000);
    }
  }

  // Frame pacing: every rAF delta is a frame time.
  let raf = 0;
  let last = performance.now();
  let scrollDir = 1;
  function frame(t: number) {
    frameTimes.push(t - last);
    last = t;
    if (scrolling && scrollEl) {
      scrollEl.scrollTop += 40 * scrollDir;
      if (scrollEl.scrollTop + scrollEl.clientHeight >= scrollEl.scrollHeight - 1) scrollDir = -1;
      else if (scrollEl.scrollTop <= 0) scrollDir = 1;
    }
    raf = requestAnimationFrame(frame);
  }
  raf = requestAnimationFrame(frame);

  const pct = (xs: number[], p: number) => {
    if (xs.length === 0) return 0;
    const s = [...xs].sort((a, b) => a - b);
    return s[Math.min(s.length - 1, Math.floor(s.length * p))];
  };

  const statsTimer = setInterval(() => {
    const f = frameTimes;
    const b = batchTimes;
    frameTimes = [];
    batchTimes = [];
    const sum = f.reduce((a, x) => a + x, 0);
    stats = {
      fps: sum ? Math.round((f.length * 1000) / sum) : 0,
      p95Frame: +pct(f, 0.95).toFixed(1),
      maxFrame: +Math.max(0, ...f).toFixed(1),
      longFrames: f.filter((x) => x > 33.4).length,
      batchMean: b.length ? +(b.reduce((a, x) => a + x, 0) / b.length).toFixed(1) : 0,
      batchP95: +pct(b, 0.95).toFixed(1),
      batchMax: +Math.max(0, ...b).toFixed(1),
    };
  }, 2000);

  onDestroy(() => {
    cancelAnimationFrame(raf);
    clearInterval(statsTimer);
    clearInterval(churnTimer);
    clearInterval(metricsTimer);
  });
</script>

<div class="bench">
  <div class="controls">
    <strong>Table benchmark</strong>
    <label>rows <input class="input" type="number" bind:value={total} min="100" step="1000" /></label>
    <button class="btn" onclick={fill}>Fill (snapshot)</button>
    <label>changes/batch <input class="input" type="number" bind:value={changesPerBatch} min="1" /></label>
    <label>batch every (ms) <input class="input" type="number" bind:value={batchMs} min="16" /></label>
    <button class="btn" class:active={churning} onclick={toggleChurn}>{churning ? "Stop churn" : "Start churn"}</button>
    <button class="btn" class:active={scrolling} onclick={() => (scrolling = !scrolling)}>
      {scrolling ? "Stop scroll" : "Auto scroll"}
    </button>
    <input class="input" type="search" placeholder="Filter…" bind:value={search} />
  </div>
  <div class="stats">
    <span>{shown} of {totalRows} rows</span>
    <span>{stats.fps} fps</span>
    <span>frame p95 {stats.p95Frame} ms</span>
    <span>frame max {stats.maxFrame} ms</span>
    <span>frames &gt;33 ms: {stats.longFrames}</span>
    <span>batch apply+render mean {stats.batchMean} ms, p95 {stats.batchP95} ms, max {stats.batchMax} ms</span>
  </div>
  <div class="log">{#each log as l (l)}<div>{l}</div>{/each}</div>
  <div class="table">
    <ResourceTable
      kind="pods"
      {store}
      {metrics}
      {now}
      {search}
      bind:scrollEl
      oncount={(s, t) => {
        shown = s;
        totalRows = t;
      }}
    />
  </div>
</div>

<style>
  .bench {
    display: flex;
    flex-direction: column;
    height: 100vh;
  }
  .controls,
  .stats {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 16px;
    padding: 8px 16px;
    border-bottom: 1px solid var(--border);
  }
  .stats {
    color: var(--text-muted);
    font-size: 12px;
  }
  .input {
    width: 90px;
  }
  .log {
    font: 11px ui-monospace, monospace;
    color: var(--text-muted);
    padding: 4px 16px;
    min-height: 40px;
    max-height: 90px;
    overflow: auto;
  }
  .table {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
</style>
