<script lang="ts">
  import { clockTime, frameFor, linePoints, valueOf, yOf } from "../usage";
  import type { ChartLine, RefLine, Resource } from "../usage";

  /** One usage chart (#47): the first line is the total, the others its
   *  containers; dashed lines are requests, limits or allocatable. */
  let {
    title,
    resource,
    lines,
    refs,
    format,
  }: {
    title: string;
    resource: Resource;
    lines: readonly ChartLine[];
    refs: readonly RefLine[];
    format: (n: number) => string;
  } = $props();

  const W = 388;
  const H = 90;
  // Containers avoid the reference colours (green, red, amber).
  const COLORS = ["var(--accent)", "#b48cf2", "#5fd4d4", "#e879a6", "#c8c8c8", "#8fa8ff"];
  const REF_COLORS: Record<string, string> = {
    request: "var(--green)",
    limit: "var(--red)",
    allocatable: "var(--amber)",
  };

  const frame = $derived(frameFor(lines, refs, resource, W, H));
  const latest = (l: ChartLine) => {
    const s = l.samples.at(-1);
    return s ? format(valueOf(s, resource)) : "—";
  };
  const peak = $derived(Math.max(0, ...(lines[0]?.samples ?? []).map((s) => valueOf(s, resource))));
</script>

<figure class="chart" data-testid="usage-chart-{resource}">
  <figcaption>
    <span class="name">{title}</span>
    <span class="muted">now {lines[0] ? latest(lines[0]) : "—"} · peak {format(peak)}</span>
  </figcaption>
  <div class="plot">
    <span class="axis top">{format(frame.max)}</span>
    <svg viewBox="0 0 {W} {H}" preserveAspectRatio="none" role="img" aria-label="{title} over time">
      <line class="grid" x1="0" x2={W} y1={H / 2} y2={H / 2} />
      {#each refs as r (r.label)}
        <line
          class="ref"
          data-ref={r.label}
          x1="0"
          x2={W}
          y1={yOf(frame, r.value)}
          y2={yOf(frame, r.value)}
          stroke={REF_COLORS[r.label] ?? "var(--text-muted)"}
        />
      {/each}
      {#each lines as l, i (l.name)}
        {#if l.samples.length > 1}
          <polyline
            data-series={l.name}
            class:total={i === 0}
            points={linePoints(frame, l.samples, resource)}
            stroke={COLORS[i % COLORS.length]}
          />
        {:else if l.samples.length === 1}
          {@const p = linePoints(frame, l.samples, resource).split(",")}
          <circle data-series={l.name} cx={p[0]} cy={p[1]} r="2" fill={COLORS[i % COLORS.length]} />
        {/if}
      {/each}
    </svg>
    <div class="axis times">
      <span>{clockTime(frame.t0)}</span>
      <span>{clockTime(frame.t1)}</span>
    </div>
  </div>
  <ul class="legend">
    {#each lines as l, i (l.name)}
      <li><span class="swatch" style:background={COLORS[i % COLORS.length]}></span>{l.name} {latest(l)}</li>
    {/each}
    {#each refs as r (r.label)}
      <li data-testid="ref-{r.label}">
        <span class="swatch dashed" style:border-color={REF_COLORS[r.label] ?? "var(--text-muted)"}></span>{r.label}
        {format(r.value)}
      </li>
    {/each}
  </ul>
</figure>

<style>
  .chart {
    margin: 0 0 12px;
  }
  figcaption {
    display: flex;
    justify-content: space-between;
    gap: 8px;
    font-size: 12px;
    margin-bottom: 4px;
  }
  .name {
    font-weight: 600;
  }
  .plot {
    position: relative;
  }
  svg {
    display: block;
    width: 100%;
    height: 90px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 4px;
  }
  polyline {
    fill: none;
    stroke-width: 1;
    stroke-linejoin: round;
    vector-effect: non-scaling-stroke;
  }
  polyline.total {
    stroke-width: 2;
  }
  line.grid {
    stroke: var(--border);
    vector-effect: non-scaling-stroke;
  }
  line.ref {
    stroke-width: 1;
    stroke-dasharray: 4 3;
    vector-effect: non-scaling-stroke;
  }
  .axis {
    font-size: 10px;
    color: var(--text-muted);
  }
  .axis.top {
    position: absolute;
    top: 2px;
    left: 4px;
    pointer-events: none;
  }
  .axis.times {
    display: flex;
    justify-content: space-between;
    margin-top: 2px;
  }
  .legend {
    list-style: none;
    display: flex;
    flex-wrap: wrap;
    gap: 2px 12px;
    margin: 4px 0 0;
    padding: 0;
    font-size: 11px;
    color: var(--text-muted);
  }
  .swatch {
    display: inline-block;
    width: 10px;
    height: 3px;
    margin-right: 4px;
    vertical-align: middle;
  }
  .swatch.dashed {
    height: 0;
    border-top: 2px dashed;
  }
</style>
