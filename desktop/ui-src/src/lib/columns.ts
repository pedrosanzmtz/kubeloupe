import type { NamespaceInfo, NodeInfo, NodeUsage, PodInfo, PodUsage } from "./bindings";
import { formatAge, formatCpu, formatMemory } from "./format";

export type Kind = "pods" | "nodes" | "namespaces";
export type Row = PodInfo | NodeInfo | NamespaceInfo;

/** Polled metrics, keyed like the rows they merge into. */
export interface Metrics {
  pods: Map<string, PodUsage>;
  nodes: Map<string, NodeUsage>;
}

/** Things a cell may depend on besides the row itself. */
export interface Ctx {
  metrics: Metrics;
  /** Wall clock (ms), ticked by the live store so ages stay fresh. */
  now: number;
}

export interface Column<R> {
  key: string;
  label: string;
  /** CSS grid track. */
  width: string;
  text(row: R, ctx: Ctx): string;
  /** Value to sort by; numbers sort numerically. */
  sort(row: R, ctx: Ctx): string | number;
  /** Optional CSS class for the cell. */
  cls?(row: R): string;
}

export function podKey(p: { namespace: string; name: string }): string {
  return `${p.namespace}/${p.name}`;
}

const phase = (r: { phase: string }) => `phase-${r.phase}`;

const POD_COLUMNS: Column<PodInfo>[] = [
  { key: "name", label: "Name", width: "minmax(160px, 2.2fr)", text: (r) => r.name, sort: (r) => r.name },
  { key: "namespace", label: "Namespace", width: "minmax(120px, 1.2fr)", text: (r) => r.namespace, sort: (r) => r.namespace },
  { key: "phase", label: "Phase", width: "minmax(80px, 0.9fr)", text: (r) => r.phase, sort: (r) => r.phase, cls: phase },
  { key: "node", label: "Node", width: "minmax(120px, 1.4fr)", text: (r) => r.node, sort: (r) => r.node },
  {
    key: "age",
    label: "Age",
    width: "64px",
    text: (r, c) => formatAge(r.created, c.now, r.age),
    sort: (r) => r.created,
  },
  {
    key: "cpu",
    label: "CPU",
    width: "80px",
    text: (r, c) => {
      const m = c.metrics.pods.get(podKey(r));
      return m ? formatCpu(m.cpu_millicores) : "—";
    },
    sort: (r, c) => c.metrics.pods.get(podKey(r))?.cpu_millicores ?? -1,
  },
  {
    key: "memory",
    label: "Memory",
    width: "90px",
    text: (r, c) => {
      const m = c.metrics.pods.get(podKey(r));
      return m ? formatMemory(m.memory_bytes) : "—";
    },
    sort: (r, c) => c.metrics.pods.get(podKey(r))?.memory_bytes ?? -1,
  },
];

const NAMESPACE_COLUMNS: Column<NamespaceInfo>[] = [
  { key: "name", label: "Name", width: "minmax(160px, 2fr)", text: (r) => r.name, sort: (r) => r.name },
  { key: "status", label: "Status", width: "minmax(100px, 1fr)", text: (r) => r.status, sort: (r) => r.status },
  {
    key: "age",
    label: "Age",
    width: "64px",
    text: (r, c) => formatAge(r.created, c.now, r.age),
    sort: (r) => r.created,
  },
];

const NODE_COLUMNS: Column<NodeInfo>[] = [
  { key: "name", label: "Name", width: "minmax(160px, 2fr)", text: (r) => r.name, sort: (r) => r.name },
  { key: "ready", label: "Ready", width: "minmax(70px, 0.7fr)", text: (r) => r.ready, sort: (r) => r.ready },
  { key: "version", label: "Version", width: "minmax(100px, 1fr)", text: (r) => r.version, sort: (r) => r.version },
  {
    key: "age",
    label: "Age",
    width: "64px",
    text: (r, c) => formatAge(r.created, c.now, r.age),
    sort: (r) => r.created,
  },
  {
    key: "cpu",
    label: "CPU",
    width: "minmax(100px, 1fr)",
    text: (r, c) => {
      const m = c.metrics.nodes.get(r.name);
      if (!m) return "—";
      return (
        formatCpu(m.cpu_millicores) +
        (m.cpu_capacity_millicores ? " / " + formatCpu(m.cpu_capacity_millicores) : "")
      );
    },
    sort: (r, c) => c.metrics.nodes.get(r.name)?.cpu_millicores ?? -1,
  },
  {
    key: "memory",
    label: "Memory",
    width: "minmax(120px, 1fr)",
    text: (r, c) => {
      const m = c.metrics.nodes.get(r.name);
      if (!m) return "—";
      return (
        formatMemory(m.memory_bytes) +
        (m.memory_capacity_bytes ? " / " + formatMemory(m.memory_capacity_bytes) : "")
      );
    },
    sort: (r, c) => c.metrics.nodes.get(r.name)?.memory_bytes ?? -1,
  },
];

const REGISTRY = {
  pods: POD_COLUMNS,
  nodes: NODE_COLUMNS,
  namespaces: NAMESPACE_COLUMNS,
} as const;

export function columnsFor(kind: Kind): Column<Row>[] {
  return REGISTRY[kind] as unknown as Column<Row>[];
}

/** Watch key of a row: pods are namespaced, the rest are cluster scoped. */
export function rowKey(kind: Kind, row: Row): string {
  return kind === "pods" ? podKey(row as PodInfo) : row.name;
}

export const EMPTY_METRICS: Metrics = { pods: new Map(), nodes: new Map() };
