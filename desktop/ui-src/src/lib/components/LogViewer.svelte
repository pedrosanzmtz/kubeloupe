<script lang="ts" module>
  let streamCounter = 0;
</script>

<script lang="ts">
  import { onDestroy, onMount, tick } from "svelte";
  import VirtualList from "./VirtualList.svelte";
  import { commands, events } from "../bindings";
  import type { LogContainer, WorkloadNotice } from "../bindings";
  import { LogBuffer, LOG_BUFFER_MAX, linePrefix, logMatches, podColor, splitHighlights } from "../logs";
  import type { TaggedText, WorkloadRef } from "../logs";
  import type { PodRef } from "../stores/detail.svelte";
  import { settings } from "../stores/settings.svelte";
  import { TAIL_OPTIONS } from "../settings";

  interface Props {
    /** Single-pod mode: the pod whose logs to show. */
    pod?: PodRef;
    /** Workload mode (instead of `pod`): merged logs of all its pods. */
    workload?: WorkloadRef;
    /** Containers of the pod; the stream starts once `ready`. Unused for workloads. */
    containers?: LogContainer[];
    ready?: boolean;
    /** An inactive dock tab: lines are buffered, not rendered, until shown. */
    hidden?: boolean;
  }

  let { pod, workload, containers = [], ready = true, hidden = false }: Props = $props();
  const isWorkload = $derived(workload !== undefined);
  const title = $derived(workload ? `${workload.namespace}/${workload.name}` : pod ? `${pod.namespace}/${pod.name}` : "");

  interface Line {
    id: number;
    text: string;
    /** Set for system messages, which are not part of the buffer. */
    cls?: string;
    /** Workload mode: the pod prefix and its colour. */
    prefix?: string;
    color?: string;
  }

  const LINE_HEIGHT = 18;
  const FILTER_DEBOUNCE_MS = 150;

  // Log options.
  let container = $state("");
  let previous = $state(false);
  // Start from the defaults on the Settings page.
  const defaults = settings.current.logs;
  let tail = $state(TAIL_OPTIONS.includes(defaults.tail) ? defaults.tail : "200");
  let since = $state("all");
  let timestamps = $state(defaults.timestamps);
  let wrap = $state(defaults.wrap);
  let follow = $state(true);
  let filterInput = $state("");
  let filter = $state("");
  let paused = $state(false);
  let statusNote = $state("");
  // Workload mode: which pods/containers have been seen, and the pod filter.
  let podFilter = $state("");
  let podStates = $state<Record<string, "live" | "ended">>({});
  let knownContainers = $state<string[]>([]);
  let truncatedNote = $state("");
  const containersByPod = new Map<string, Set<string>>();
  let tagInfoCache = new Map<number, { prefix: string; lower: string; color: string }>();

  // The ring buffer and the lines waiting for the next frame live outside
  // reactive state; only the displayed lines are signalled, by `rev`.
  const buf = new LogBuffer();
  let pending: string[] = [];
  let pendingTags: number[] = [];
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
  /** Whether `streamId` has been started (and must be stopped). */
  let streaming = false;

  const filterLower = $derived(filter.toLowerCase());
  const count = $derived.by(() => {
    void rev;
    return d.lines.length;
  });
  const keyOf = $derived.by(() => {
    void rev;
    return (i: number) => d.lines[i]?.id ?? i;
  });
  const podNames = $derived(Object.keys(podStates).sort());
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
    // Lines that came before it go first.
    if (pending.length > 0) {
      cancelAnimationFrame(raf);
      flush();
    }
    const atBottom = isAtBottom();
    d.lines.push({ id: nextLineId++, text, cls });
    rev++;
    followIfNeeded(atBottom);
  }

  /** Prefix, lowercase prefix (for filtering) and colour of a workload line's tag. */
  function tagInfo(tagId: number) {
    let info = tagInfoCache.get(tagId);
    if (!info) {
      const tag = buf.tags[tagId];
      const multi = (containersByPod.get(tag.pod)?.size ?? 0) > 1;
      const prefix = linePrefix(tag, multi);
      info = { prefix, lower: prefix.toLowerCase(), color: podColor(tag.pod) };
      tagInfoCache.set(tagId, info);
    }
    return info;
  }

  /** Whether a buffered line is shown under the pod filter and the text filter. */
  function passes(text: string, tagId: number): boolean {
    if (!tagId) return logMatches(text, filterLower);
    if (podFilter && buf.tags[tagId].pod !== podFilter) return false;
    // The filter also matches the pod (and container) name.
    return !filterLower || tagInfo(tagId).lower.includes(filterLower) || logMatches(text, filterLower);
  }

  function makeLine(text: string, tagId: number): Line {
    if (!tagId) return { id: nextLineId++, text };
    const info = tagInfo(tagId);
    return { id: nextLineId++, text, prefix: info.prefix, color: info.color };
  }

  function flush() {
    raf = 0;
    if (paused || pending.length === 0) return;
    const lines = pending;
    const tags = pendingTags;
    pending = [];
    pendingTags = [];
    const atBottom = isAtBottom();
    for (let i = 0; i < lines.length; i++) {
      if (passes(lines[i], tags[i])) d.lines.push(makeLine(lines[i], tags[i]));
    }
    if (d.lines.length > LOG_BUFFER_MAX + LOG_BUFFER_MAX / 10) {
      d.lines.splice(0, d.lines.length - LOG_BUFFER_MAX);
    }
    rev++;
    updateStatus();
    followIfNeeded(atBottom);
  }

  /** Queue lines for the next frame; a hidden tab only renders when shown. */
  function schedule() {
    if (hidden) {
      // Only the newest LOG_BUFFER_MAX can end up on screen anyway.
      if (pending.length > LOG_BUFFER_MAX + LOG_BUFFER_MAX / 10) {
        pending.splice(0, pending.length - LOG_BUFFER_MAX);
        pendingTags.splice(0, pendingTags.length - LOG_BUFFER_MAX);
      }
      return;
    }
    if (!raf) raf = requestAnimationFrame(flush);
  }

  function onBatch(lines: string[]) {
    buf.push(lines);
    if (paused) {
      updateStatus();
      return;
    }
    for (const l of lines) {
      pending.push(l);
      pendingTags.push(0);
    }
    schedule();
  }

  /** Remember a pod/container of a workload stream (for the pickers and prefixes). */
  function notePod(podName: string, containerName: string, state: "live" | "ended" = "live") {
    if (podStates[podName] !== state) podStates[podName] = state;
    let set = containersByPod.get(podName);
    if (!set) containersByPod.set(podName, (set = new Set()));
    if (!set.has(containerName)) {
      set.add(containerName);
      if (set.size === 2) tagInfoCache = new Map(); // prefixes now include the container
    }
    if (containerName && !knownContainers.includes(containerName)) {
      knownContainers = [...knownContainers, containerName].sort();
    }
  }

  function onTaggedBatch(lines: TaggedText[]) {
    for (const l of lines) {
      if (podStates[l.pod] === undefined || !containersByPod.get(l.pod)?.has(l.container)) {
        notePod(l.pod, l.container);
      }
    }
    buf.pushTagged(lines);
    if (paused) {
      updateStatus();
      return;
    }
    const n = lines.length;
    const total = buf.lines.length;
    for (let i = total - n; i < total; i++) {
      pending.push(buf.lines[i]);
      pendingTags.push(buf.tagIds[i]);
    }
    schedule();
  }

  function onNotice(n: WorkloadNotice) {
    const who = `${n.pod}/${n.container}`;
    switch (n.kind) {
      case "started":
        notePod(n.pod, n.container);
        appendLog(`+ ${who} started`, "muted");
        break;
      case "ended":
      case "deleted":
        if (podStates[n.pod] !== undefined) podStates[n.pod] = "ended";
        appendLog(`- ${who} ${n.message}`, "muted");
        break;
      case "failed":
        appendLog(`! ${who}: ${n.message}`, "error-text");
        break;
      case "truncated":
        truncatedNote = n.skipped > 0 ? n.message : "";
        break;
    }
  }

  /** Rebuild the display from the buffer (filter change, resume). */
  function renderLogs() {
    pending = [];
    pendingTags = [];
    d.lines = [];
    for (let i = 0; i < buf.lines.length; i++) {
      const text = buf.lines[i];
      if (passes(text, buf.tagIds[i])) d.lines.push(makeLine(text, buf.tagIds[i]));
    }
    rev++;
    updateStatus();
    followIfNeeded(true);
  }

  function resetLogs() {
    buf.clear();
    pending = [];
    pendingTags = [];
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
    // Each tab owns its stream: stop the one it replaces (it counts towards
    // the backend's cap until then).
    const old = streaming ? streamId : null;
    streamId = ++streamCounter;
    streaming = true;
    if (old !== null) await commands.cmdStopLogs(old).catch(() => {});
    try {
      if (workload) {
        podStates = {};
        containersByPod.clear();
        tagInfoCache = new Map();
        truncatedNote = "";
        await commands.cmdStreamWorkloadLogs(
          workload.kind,
          workload.namespace,
          workload.name,
          {
            container: container || null,
            tailLines: tail === "all" ? null : Number(tail),
            sinceSeconds: since === "all" ? null : Number(since),
            timestamps,
          },
          streamId,
        );
        return;
      }
      if (!pod) return;
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
    const name = container || (workload ? "all" : "pod");
    const stamp = new Date().toISOString().replace(/[:.]/g, "-");
    const filename = `${(workload ?? pod)?.name}_${name}${previous ? "_previous" : ""}_${stamp}.log`;
    try {
      // Written by the backend: Blob/anchor downloads are not reliable in
      // Tauri's webviews, and a save dialog would need an extra plugin.
      const text = isWorkload
        ? buf.lines.map((l, i) => (buf.tagIds[i] ? tagInfo(buf.tagIds[i]).prefix + " " + l : l)).join("\n")
        : buf.lines.join("\n");
      const path = await commands.cmdSaveLogs(filename, text + "\n");
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
    // Workloads follow all containers by default.
    const firstMain = isWorkload ? undefined : containers.find((c) => !c.init);
    container = firstMain?.name ?? "";
    startLogs();
  });

  // The stream restarts when an option changes (but not for wrap/filter/follow).
  async function restart() {
    // `bind:` and `onchange` both run on change; wait for the bound value.
    await tick();
    if (started) startLogs();
  }

  async function renderSoon() {
    await tick();
    renderLogs();
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
      events.workloadLog.listen((e) => e.payload.streamId === streamId && onTaggedBatch(e.payload.data)),
      events.workloadNotice.listen((e) => e.payload.streamId === streamId && onNotice(e.payload)),
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
    const ownStream = streamId;
    streamId = ++streamCounter;
    // Stops this tab's stream only; the other tabs keep theirs.
    if (streaming) commands.cmdStopLogs(ownStream).catch(() => {});
  });

  // A tab that is shown again renders what arrived while it was hidden.
  $effect(() => {
    if (!hidden && pending.length > 0 && !raf) raf = requestAnimationFrame(flush);
  });
</script>

<div class="logs-header section-header">
  <h3>Logs<span class="muted"> · {title}</span></h3>
  <label class="check"><input type="checkbox" bind:checked={follow} /> Follow</label>
  <button type="button" class="btn" class:active={paused} onclick={() => setPaused(!paused)}>
    {paused ? "Resume" : "Pause"}
  </button>
  <button type="button" class="btn" onclick={resetLogs}>Clear</button>
  <button type="button" class="btn" onclick={downloadLogs}>Download</button>
</div>
<div class="logs-options">
  {#if isWorkload}
    <select class="mini" title="Pod" bind:value={podFilter} onchange={renderSoon}>
      <option value="">All pods ({podNames.length})</option>
      {#each podNames as p (p)}
        <option value={p}>{p}{podStates[p] === "ended" ? " (ended)" : ""}</option>
      {/each}
    </select>
    <select class="mini" title="Container" bind:value={container} onchange={restart}>
      <option value="">All containers</option>
      {#each knownContainers as c (c)}
        <option value={c}>{c}</option>
      {/each}
    </select>
  {:else}
    <select class="mini" title="Container" bind:value={container} onchange={restart}>
      {#each containers as c (c.name)}
        <option value={c.name}>{c.init ? `${c.name} (init)` : c.name}</option>
      {/each}
    </select>
    <label class="check"><input type="checkbox" bind:checked={previous} onchange={restart} /> Previous</label>
  {/if}
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
  {#if truncatedNote}<span class="warn" title="At most 20 containers are followed at once">{truncatedNote}</span>{/if}
</div>
<div class="output" data-testid="log-output" style:--line-height="{LINE_HEIGHT}px">
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
          <div class="line {line.cls ?? ''}" class:wrap data-testid="log-line">
            {#if line.prefix}<span class="pod" style:color={line.color}
                >{#each splitHighlights(line.prefix, filter) as seg}{#if seg.hit}<mark>{seg.text}</mark
                    >{:else}{seg.text}{/if}{/each}</span
              >{/if}{#each splitHighlights(line.text, line.cls ? '' : filter) as seg}{#if seg.hit}<mark>{seg.text}</mark
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
    flex: 1;
    min-height: 40px;
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
  .pod {
    margin-right: 8px;
    font-weight: 600;
  }
  .warn {
    color: var(--amber);
  }
  .line.error-text {
    color: #f5a3a5;
  }
</style>
