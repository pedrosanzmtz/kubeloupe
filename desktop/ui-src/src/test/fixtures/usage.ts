// Usage history (#47) of the fake cluster: what `cmd_usage_history` returns
// and the sparklines `cmd_get_metrics` sends.

import type { Spark, UsageHistoryView, UsageReference, UsageSample } from "../../lib/bindings";

const MI = 1024 * 1024;
const STEP_MS = 20_000;

export const NO_REFERENCE: UsageReference = {
  cpu_request: 0,
  cpu_limit: 0,
  memory_request: 0,
  memory_limit: 0,
  cpu_allocatable: 0,
  memory_allocatable: 0,
};

/** `n` samples 20 s apart ending now, from `cpu(i)` and `mem(i)`. */
function series(n: number, cpu: (i: number) => number, mem: (i: number) => number): UsageSample[] {
  const end = Date.now();
  return Array.from({ length: n }, (_, i) => ({
    t: end - (n - 1 - i) * STEP_MS,
    cpu_millicores: Math.round(cpu(i)),
    memory_bytes: Math.round(mem(i)),
  }));
}

function sum(a: UsageSample[], b: UsageSample[]): UsageSample[] {
  return a.map((s, i) => ({
    t: s.t,
    cpu_millicores: s.cpu_millicores + b[i].cpu_millicores,
    memory_bytes: s.memory_bytes + b[i].memory_bytes,
  }));
}

/** By `pods|namespace/name` or `nodes|name`; others have no history. */
export function usageFixtures(): Record<string, UsageHistoryView> {
  // web-1: memory climbing towards its 128Mi limit.
  const app = series(
    12,
    (i) => 80 + 10 * Math.sin(i),
    (i) => (40 + 5 * i) * MI,
  );
  const sidecar = series(
    12,
    () => 5,
    () => 8 * MI,
  );
  const web1 = sum(app, sidecar);
  const node = series(
    12,
    (i) => 700 + 20 * i,
    () => 2 * 1024 * MI,
  );
  return {
    "pods|demo/web-1": {
      history: {
        since: web1[0].t,
        samples: web1,
        containers: [
          { name: "app", samples: app },
          { name: "sidecar", samples: sidecar },
        ],
      },
      // The pod's requests and limits, from its spec: app has 100m/64Mi
      // requests and a 128Mi memory limit; the sidecar has none.
      reference: { ...NO_REFERENCE, cpu_request: 100, memory_request: 64 * MI },
    },
    "nodes|node-1": {
      history: { since: node[0].t, samples: node, containers: [] },
      reference: {
        ...NO_REFERENCE,
        cpu_allocatable: 3800,
        memory_allocatable: 7 * 1024 * MI,
      },
    },
  };
}

/** Sparklines of the table rows that have history (keyed like the rows). */
export function sparkFixtures(): Record<string, Spark> {
  const u = usageFixtures();
  const spark = (s: UsageSample[]): Spark => ({
    cpu: s.map((x) => x.cpu_millicores),
    memory: s.map((x) => x.memory_bytes),
  });
  return {
    "demo/web-1": spark(u["pods|demo/web-1"].history.samples),
    "node-1": spark(u["nodes|node-1"].history.samples),
  };
}
