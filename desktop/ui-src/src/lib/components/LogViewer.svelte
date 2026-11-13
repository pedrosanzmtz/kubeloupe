<script lang="ts" module>
  let streamCounter = 0;
</script>

<script lang="ts">
  import { onDestroy, onMount, tick } from "svelte";
  import VirtualList from "./VirtualList.svelte";
  import { commands, events } from "../bindings";
  import type { LogContainer } from "../bindings";
  import { LogBuffer, LOG_BUFFER_MAX, logMatches, splitHighlights } from "../logs";
  import { detail } from "../stores/detail.svelte";
  import type { PodRef } from "../stores/detail.svelte";

  interface Props {
    pod: PodRef;
    /** Containers of the pod; the stream starts once `ready`. */
    containers: LogContainer[];
    ready: boolean;
  }

  let { pod, containers, ready }: Props = $props();

  interface Line {
    id: number;
    text: string;
    /** Set for system messages, which are not part of the buffer. */
    cls?: string;
  }

  const LINE_HEIGHT = 18;
  const FILTER_DEBOUNCE_MS = 150;

  // Log options.
  let container = $state("");
  let previous = $state(false);
  let tail = $state("200");
  let since = $state("all");
  let timestamps = $state(false);
  let wrap = $state(false);
  let follow = $state(true);
  let filterInput = $state("");
  let filter = $state("");
  let paused = $state(false);
  let statusNote = $state("");

  // The ring buffer and the lines waiting for the next frame live outside
  // reactive state; only the displayed lines are signalled, by `rev`.
  const buf = new LogBuffer();
  let pending: string[] = [];
  const d = { lines: [] as Line[] };
  let rev = $state(0);
  let lineCount = $state(0);
  let nextLineId = 0;
  let raf = 0;
  let filterTimer: ReturnType<typeof setTimeout> | undefined;

  let list = $state<ReturnType<typeof VirtualList>>();
  let scrollEl = $state<HTMLDivElement | null>(null);

  // Events carry the id of their stream; ids only grow, so a straggler from a
  // replaced stream never matches.
  let streamId = ++streamCounter;

  const filterLower = $derived(filter.toLowerCase());
  const count = $derived.by(() => {
    void rev;
    return d.lines.length;
  });
  const keyOf = $derived.by(() => {
    void rev;
    return (i: number) => d.lines[i]?.id ?? i;
  });
  const status = $derived(
    statusNote ||
      (paused ? "paused · " : "") + lineCount.toLocaleString() + (lineCount >= LOG_BUFFER_MAX ? "+" : "") + " lines",
  );

  function isAtBottom(): boolean {
    return !scrollEl || scrollEl.scrollHeight - scrollEl.scrollTop - scrollEl.clientHeight < 40;
  }

  async function followIfNeeded(wasAtBottom: boolean) {
    if (follow && wasAtBottom) {
      await tick();
      list?.scrollToBottom();
    }
  }

  function updateStatus() {
    statusNote = "";
    lineCount = buf.lines.length;
  }

  /** Append a system message (error, "stream ended"); it is not in the buffer. */
  function appendLog(text: string, cls?: string) {
    const atBottom = isAtBottom();
    d.lines.push({ id: nextLineId++, text, cls });
    rev++;
    followIfNeeded(atBottom);
  }

  function flush() {
    raf = 0;
    if (paused || pending.length === 0) return;
    const lines = pending;
    pending = [];
    const atBottom = isAtBottom();
    for (const text of lines) {
      if (logMatches(text, filterLower)) d.lines.push({ id: nextLineId++, text });
    }
    if (d.lines.length > LOG_BUFFER_MAX + LOG_BUFFER_MAX / 10) {
      d.lines.splice(0, d.lines.length - LOG_BUFFER_MAX);
    }
    rev++;
    updateStatus();
    followIfNeeded(atBottom);
  }

  function onBatch(lines: string[]) {
    buf.push(lines);
    if (paused) {
      updateStatus();
      return;
    }
    for (const l of lines) pending.push(l);
    if (!raf) raf = requestAnimationFrame(flush);
  }

  /** Rebuild the display from the buffer (filter change, resume). */
  function renderLogs() {
    pending = [];
    d.lines = [];
    for (const text of buf.lines) {
      if (logMatches(text, filterLower)) d.lines.push({ id: nextLineId++, text });
    }
    rev++;
    updateStatus();
    followIfNeeded(true);
  }

  function resetLogs() {
    buf.clear();
    pending = [];
    d.lines = [];
    rev++;
    updateStatus();
  }

  function setPaused(value: boolean) {
    paused = value;
    if (!paused) renderLogs();
    else updateStatus();
  }

  async function startLogs() {
    resetLogs();
    setPaused(false);
    streamId = ++streamCounter;
    try {
      await commands.cmdStreamPodLogs(
        pod.namespace,
        pod.name,
        container || null,
        previous,
        tail === "all" ? null : Number(tail),
        since === "all" ? null : Number(since),
        timestamps,
        streamId,
      );
    } catch (e) {
      appendLog("Could not start log stream: " + e, "error-text");
    }
  }

  async function downloadLogs() {
    const name = container || "pod";
    const stamp = new Date().toISOString().replace(/[:.]/g, "-");
    const filename = `${pod.name}_${name}${previous ? "_previous" : ""}_${stamp}.log`;
    try {
      // Written by the backend: Blob/anchor downloads are not reliable in
      // Tauri's webviews, and a save dialog would need an extra plugin.
      const path = await commands.cmdSaveLogs(filename, buf.lines.join("\n") + "\n");
      statusNote = "saved to " + path;
    } catch (e) {
      appendLog("Could not save logs: " + e, "error-text");
    }
  }

  function onFilterInput() {
    clearTimeout(filterTimer);
    filterTimer = setTimeout(() => {
      filter = filterInput;
      renderLogs();
    }, FILTER_DEBOUNCE_MS);
  }

  // Start streaming once the pod's containers are known (or failed to load).
  let started = false;
  $effect(() => {
    if (!ready || started) return;
    started = true;
    const firstMain = containers.find((c) => !c.init);
    container = firstMain?.name ?? "";
    startLogs();
  });

  // The stream restarts when an option changes (but not for wrap/filter/follow).
  async function restart() {
    // `bind:` and `onchange` both run on change; wait for the bound value.
    await tick();
    if (started) startLogs();
  }

  async function toggleWrap() {
    await tick();
    followIfNeeded(true);
  }

  let unlisteners: (() => void)[] = [];
  let destroyed = false;
  onMount(async () => {
    const un = await Promise.all([
      events.podLog.listen((e) => e.payload.streamId === streamId && onBatch(e.payload.data)),
      events.podLogDone.listen((e) => e.payload.streamId === streamId && appendLog("[stream ended]", "muted")),
      events.podLogError.listen((e) => e.payload.streamId === streamId && appendLog(e.payload.data, "error-text")),
    ]);
    if (destroyed) un.forEach((f) => f());
    else unlisteners = un;
  });

  onDestroy(() => {
    destroyed = true;
    unlisteners.forEach((f) => f());
    cancelAnimationFrame(raf);
    clearTimeout(filterTimer);
    streamId = ++streamCounter;
    // When another pod replaced this one, its stream start already stops ours;
    // stopping here could race with it and kill the new stream. Decide after
    // the current update has settled: during teardown `detail.pod` may still
    // hold the previous value.
    queueMicrotask(() => {
      if (!detail.pod) commands.cmdStopPodLogs().catch(() => {});
    });
  });
</script>

<div class="logs-header section-header">
  <h3>Logs</h3>
  <label class="check"><input type="checkbox" bind:checked={follow} /> Follow</label>
  <button type="button" class="btn" class:active={paused} onclick={() => setPaused(!paused)}>
    {paused ? "Resume" : "Pause"}
  </button>
  <button type="button" class="btn" onclick={resetLogs}>Clear</button>
  <button type="button" class="btn" onclick={downloadLogs}>Download</button>
</div>
<div class="logs-options">
  <select class="mini" title="Container" bind:value={container} onchange={restart}>
    {#each containers as c (c.name)}
      <option value={c.name}>{c.init ? `${c.name} (init)` : c.name}</option>
    {/each}
  </select>
  <label class="check"><input type="checkbox" bind:checked={previous} onchange={restart} /> Previous</label>
  <select class="mini" title="Tail lines" bind:value={tail} onchange={restart}>
    <option value="100">100 lines</option>
    <option value="200">200 lines</option>
    <option value="1000">1000 lines</option>
    <option value="5000">5000 lines</option>
    <option value="all">All lines</option>
  </select>
  <select class="mini" title="Since" bind:value={since} onchange={restart}>
    <option value="all">All time</option>
    <option value="300">Last 5m</option>
    <option value="3600">Last 1h</option>
  </select>
  <label class="check"><input type="checkbox" bind:checked={timestamps} onchange={restart} /> Timestamps</label>
  <label class="check"><input type="checkbox" bind:checked={wrap} onchange={toggleWrap} /> Wrap</label>
  <input
    class="mini filter"
    type="search"
    placeholder="Filter logs…"
    bind:value={filterInput}
    oninput={onFilterInput}
  />
  <span class="muted">{status}</span>
</div>
<div class="output" style:--line-height="{LINE_HEIGHT}px">
  {#key wrap}
    <VirtualList
      bind:this={list}
      bind:scrollEl
      {count}
      rowHeight={LINE_HEIGHT}
      dynamic={wrap}
      scrollX={!wrap}
      overscan={20}
      getKey={keyOf}
    >
      {#snippet row(i)}
        {@const line = d.lines[i]}
        {#if line}
          <div class="line {line.cls ?? ''}" class:wrap>
            {#each splitHighlights(line.text, line.cls ? '' : filter) as seg}{#if seg.hit}<mark>{seg.text}</mark
                >{:else}{seg.text}{/if}{/each}
          </div>
        {/if}
      {/snippet}
    </VirtualList>
  {/key}
</div>

<style>
  .logs-header h3 {
    margin: 0;
    font-size: 13px;
    flex: 1;
  }
  .logs-options {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 12px;
    padding: 6px 16px;
    font-size: 12px;
  }
  .check {
    color: var(--text-muted);
    font-size: 12px;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .mini {
    background: transparent;
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 3px 6px;
    font-size: 12px;
  }
  .mini option {
    background: var(--bg-raised);
  }
  .filter {
    flex: 1;
    min-width: 120px;
  }
  .output {
    display: flex;
    flex-direction: column;
    flex: none;
    height: 200px;
    min-height: 120px;
    background: #0d1013;
    color: #d6dae0;
    border-top: 1px solid var(--border);
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 12px;
    padding: 0 0 0 16px;
  }
  .line {
    min-height: var(--line-height);
    line-height: var(--line-height);
    white-space: pre;
    width: max-content;
    min-width: 100%;
  }
  .line.wrap {
    white-space: pre-wrap;
    word-break: break-all;
    width: auto;
  }
  .line :global(mark),
  mark {
    background: #8a6d00;
    color: inherit;
    border-radius: 2px;
  }
  .muted {
    color: var(--text-muted);
  }
  .line.muted {
    color: var(--text-muted);
  }
  .line.error-text {
    color: #f5a3a5;
  }
</style>
