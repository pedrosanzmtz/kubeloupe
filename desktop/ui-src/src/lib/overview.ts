import type { Capacity, EventInfo, PodCounts, ResourceKind } from "./bindings";
import { formatCpu, formatMemory } from "./format";

/** One bar of the capacity section. */
export interface CapacityBar {
  label: string;
  /** "1.2 / 8 requested" */
  text: string;
  /** 0–100, capped; what the bar fills to. */
  percent: number;
  /** Usage from metrics, when available ("900m used, 11%"). */
  usage: string;
  /** Over 90% of what can be allocated. */
  high: boolean;
}

export function percent(used: number, total: number): number {
  if (!total) return 0;
  return Math.min(100, Math.round((used / total) * 100));
}

/** CPU, memory and pod bars, Lens style: requests against allocatable. */
export function capacityBars(c: Capacity): CapacityBar[] {
  const bar = (label: string, used: number, total: number, fmt: (n: number) => string, usage: number | null) => {
    const p = percent(used, total);
    return {
      label,
      text: `${fmt(used)} / ${fmt(total)} requested (${p}%)`,
      percent: p,
      usage: usage === null ? "" : `${fmt(usage)} used (${percent(usage, total)}%)`,
      high: p > 90,
    };
  };
  const cpuUsage = c.metrics ? c.cpu_usage_millicores : null;
  const memUsage = c.metrics ? c.memory_usage_bytes : null;
  const pods = percent(c.pods, c.pods_allocatable);
  return [
    bar("CPU", c.cpu_requests_millicores, c.cpu_allocatable_millicores, formatCpu, cpuUsage),
    bar("Memory", c.memory_requests_bytes, c.memory_allocatable_bytes, formatMemory, memUsage),
    {
      label: "Pods",
      text: `${c.pods} / ${c.pods_allocatable} (${pods}%)`,
      percent: pods,
      usage: "",
      high: pods > 90,
    },
  ];
}

/** The status cards, in order. `alert` marks counts that need attention. */
export function podCards(p: PodCounts): { label: string; value: number; alert: boolean; title: string }[] {
  return [
    { label: "Running", value: p.running, alert: false, title: "Running, without a waiting error" },
    { label: "Pending", value: p.pending, alert: false, title: "Pending (scheduling, pulling, starting)" },
    {
      label: "Errors",
      value: p.waiting_errors,
      alert: p.waiting_errors > 0,
      title: "A container waits on an error: CrashLoopBackOff, ImagePullBackOff, …",
    },
    { label: "Failed", value: p.failed, alert: p.failed > 0, title: "Phase Failed (including evicted pods)" },
    { label: "Succeeded", value: p.succeeded, alert: false, title: "Completed (Jobs)" },
  ];
}

/**
 * The kind an event's object belongs to, if discovery knows it. Events
 * carry `apiVersion` + `kind`; the version may be one discovery does not
 * prefer, so fall back to the group alone.
 */
export function eventKind(e: EventInfo, kinds: ResourceKind[]): ResourceKind | undefined {
  if (!e.object_kind || !e.object_name) return undefined;
  const v = e.object_api_version || "v1";
  const [group, version] = v.includes("/") ? v.split("/") : ["", v];
  const same = (k: ResourceKind) => k.resource.group === group && k.resource.kind === e.object_kind;
  return kinds.find((k) => same(k) && k.resource.version === version) ?? kinds.find(same);
}

/** The footer count: "5 pods, 2 nodes". */
export function overviewCount(pods: PodCounts | null, nodes: { total: number } | null): string {
  const parts = [];
  if (pods) parts.push(`${pods.total} pods`);
  if (nodes) parts.push(`${nodes.total} nodes`);
  return parts.join(", ");
}
