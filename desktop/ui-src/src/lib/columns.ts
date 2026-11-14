import type { ColumnDef, NodeUsage, PodUsage, ResourceKind, ResourceRow, Spark } from "./bindings";
import { formatAge, formatCpu, formatMemory } from "./format";

export type Row = ResourceRow;

/** Polled metrics, keyed like the rows they merge into. */
export interface Metrics {
  pods: Map<string, PodUsage>;
  nodes: Map<string, NodeUsage>;
  /** Usage history for the sparklines (#47), keyed like `pods` or `nodes`. */
  sparks: Map<string, Spark>;
  /** Epoch ms the history of what is shown starts at (0 = none yet). */
  since: number;
}

/** Things a cell may depend on besides the row itself. */
export interface Ctx {
  metrics: Metrics;
  /** Wall clock (ms), ticked by the live store so ages stay fresh. */
  now: number;
}

export interface Column {
  key: string;
  label: string;
  /** CSS grid track. */
  width: string;
  text(row: Row, ctx: Ctx): string;
  /** Value to sort by; numbers sort numerically. */
  sort(row: Row, ctx: Ctx): string | number;
  /** Optional CSS class for the cell. */
  cls?(row: Row): string;
  /** Values of the cell's sparkline, oldest first (metric columns only). */
  spark?(row: Row, ctx: Ctx): readonly number[];
}

export const EMPTY_METRICS: Metrics = { pods: new Map(), nodes: new Map(), sparks: new Map(), since: 0 };
const NO_SPARK: readonly number[] = [];

export function isPod(kind: ResourceKind): boolean {
  return kind.resource.group === "" && kind.resource.kind === "Pod";
}
export function isNode(kind: ResourceKind): boolean {
  return kind.resource.group === "" && kind.resource.kind === "Node";
}

function widthFor(def: ColumnDef): string {
  switch (def.kind) {
    case "age":
      return "64px";
    case "podCpu":
      return "112px";
    case "podMemory":
      return "122px";
    case "nodeCpu":
      return "minmax(140px, 1fr)";
    case "nodeMemory":
      return "minmax(160px, 1fr)";
  }
  if (def.key === "name") return "minmax(160px, 2.2fr)";
  if (def.key === "namespace") return "minmax(120px, 1.2fr)";
  if (def.key === "message") return "minmax(240px, 3fr)";
  if (def.kind === "status") return "minmax(120px, 1fr)";
  return "minmax(80px, 1fr)";
}

/** Number to sort a numeric cell by ("3", or the leading number of "1/2"). */
function numeric(text: string): number {
  const n = parseFloat(text);
  return Number.isNaN(n) ? -1 : n;
}

function column(def: ColumnDef, index: number): Column {
  const base = { key: def.key, label: def.label, width: widthFor(def) };
  const cell = (r: Row) => r.cells[index] ?? "";
  switch (def.kind) {
    case "age":
      return {
        ...base,
        text: (r, c) => formatAge(r.created, c.now, ""),
        sort: (r) => r.created,
      };
    case "podCpu":
      return {
        ...base,
        text: (r, c) => {
          const m = c.metrics.pods.get(r.key);
          return m ? formatCpu(m.cpu_millicores) : "—";
        },
        sort: (r, c) => c.metrics.pods.get(r.key)?.cpu_millicores ?? -1,
        spark: (r, c) => c.metrics.sparks.get(r.key)?.cpu ?? NO_SPARK,
      };
    case "podMemory":
      return {
        ...base,
        text: (r, c) => {
          const m = c.metrics.pods.get(r.key);
          return m ? formatMemory(m.memory_bytes) : "—";
        },
        sort: (r, c) => c.metrics.pods.get(r.key)?.memory_bytes ?? -1,
        spark: (r, c) => c.metrics.sparks.get(r.key)?.memory ?? NO_SPARK,
      };
    case "nodeCpu":
      return {
        ...base,
        text: (r, c) => {
          const m = c.metrics.nodes.get(r.name);
          if (!m) return "—";
          return (
            formatCpu(m.cpu_millicores) +
            (m.cpu_capacity_millicores ? " / " + formatCpu(m.cpu_capacity_millicores) : "")
          );
        },
        sort: (r, c) => c.metrics.nodes.get(r.name)?.cpu_millicores ?? -1,
        spark: (r, c) => c.metrics.sparks.get(r.name)?.cpu ?? NO_SPARK,
      };
    case "nodeMemory":
      return {
        ...base,
        text: (r, c) => {
          const m = c.metrics.nodes.get(r.name);
          if (!m) return "—";
          return (
            formatMemory(m.memory_bytes) +
            (m.memory_capacity_bytes ? " / " + formatMemory(m.memory_capacity_bytes) : "")
          );
        },
        sort: (r, c) => c.metrics.nodes.get(r.name)?.memory_bytes ?? -1,
        spark: (r, c) => c.metrics.sparks.get(r.name)?.memory ?? NO_SPARK,
      };
    case "number":
      return { ...base, text: cell, sort: (r) => numeric(cell(r)) };
    case "status":
      return { ...base, text: cell, sort: cell, cls: (r) => `phase-${cell(r)}` };
    default:
      return { ...base, text: cell, sort: cell };
  }
}

const cache = new WeakMap<ResourceKind, Column[]>();

/**
 * Columns of a kind. Which columns exist, and how their cells are computed,
 * is declared by the backend's registry (`src/resources.rs`) and arrives with
 * discovery; this only decides how to render and sort each kind of column.
 */
export function columnsFor(kind: ResourceKind): Column[] {
  let cols = cache.get(kind);
  if (!cols) {
    cols = kind.columns.map(column);
    cache.set(kind, cols);
  }
  return cols;
}
