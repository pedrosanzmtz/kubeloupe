import { describe, expect, it } from "vitest";
import type { Capacity, EventInfo, PodCounts, ResourceKind } from "./bindings";
import { capacityBars, eventKind, overviewCount, percent, podCards } from "./overview";

const capacity: Capacity = {
  cpu_requests_millicores: 1200,
  cpu_allocatable_millicores: 8000,
  cpu_usage_millicores: 900,
  memory_requests_bytes: 3 * 1024 ** 3,
  memory_allocatable_bytes: 16 * 1024 ** 3,
  memory_usage_bytes: 4 * 1024 ** 3,
  metrics: true,
  pods: 105,
  pods_allocatable: 110,
};

const kind = (group: string, version: string, k: string, namespaced = true): ResourceKind => ({
  resource: { group, version, kind: k, plural: k.toLowerCase() + "s", namespaced },
  id: `${group ? group + "/" : ""}${version}/${k.toLowerCase()}s`,
  label: k + "s",
  category: "workloads",
  crd: false,
  verbs: [],
  short_names: [],
  columns: [],
  actions: [],
});

const event = (apiVersion: string, k: string, name = "web"): EventInfo => ({
  key: "demo/e1",
  kind: "Warning",
  reason: "BackOff",
  message: "",
  object: `${k}/${name}`,
  object_api_version: apiVersion,
  object_kind: k,
  object_namespace: "demo",
  object_name: name,
  time: 0,
  count: 1,
});

describe("percent", () => {
  it("rounds, caps at 100 and survives a zero total", () => {
    expect(percent(1, 3)).toBe(33);
    expect(percent(5, 4)).toBe(100);
    expect(percent(5, 0)).toBe(0);
  });
});

describe("capacityBars", () => {
  it("shows requests against allocatable, with usage when metrics are there", () => {
    const [cpu, mem, pods] = capacityBars(capacity);
    expect(cpu.text).toBe("1.2 / 8 requested (15%)");
    expect(cpu.usage).toBe("900m used (11%)");
    expect(mem.text).toBe("3Gi / 16Gi requested (19%)");
    expect(mem.usage).toBe("4Gi used (25%)");
    expect(pods.text).toBe("105 / 110 (95%)");
    expect(pods.high).toBe(true);
    expect(cpu.high).toBe(false);
  });

  it("leaves usage out without metrics", () => {
    const [cpu, mem] = capacityBars({ ...capacity, metrics: false });
    expect(cpu.usage).toBe("");
    expect(mem.usage).toBe("");
  });
});

describe("podCards", () => {
  it("flags waiting errors and failures", () => {
    const counts: PodCounts = {
      namespace: null,
      total: 5,
      running: 3,
      pending: 1,
      succeeded: 0,
      failed: 0,
      waiting_errors: 1,
      by_namespace: [],
      problems: [],
      problems_total: 0,
    };
    const cards = podCards(counts);
    expect(cards.map((c) => [c.label, c.value, c.alert])).toEqual([
      ["Running", 3, false],
      ["Pending", 1, false],
      ["Errors", 1, true],
      ["Failed", 0, false],
      ["Succeeded", 0, false],
    ]);
    expect(overviewCount(counts, { total: 2 })).toBe("5 pods, 2 nodes");
    expect(overviewCount(counts, null)).toBe("5 pods");
  });
});

describe("eventKind", () => {
  const kinds = [kind("", "v1", "Pod"), kind("apps", "v1", "Deployment"), kind("", "v1", "Node", false)];

  it("finds the kind by group and version, or by group alone", () => {
    expect(eventKind(event("v1", "Pod"), kinds)?.id).toBe("v1/pods");
    expect(eventKind(event("apps/v1", "Deployment"), kinds)?.id).toBe("apps/v1/deployments");
    expect(eventKind(event("apps/v1beta2", "Deployment"), kinds)?.id).toBe("apps/v1/deployments");
  });

  it("gives up on unknown kinds and objects without a name", () => {
    expect(eventKind(event("example.com/v1", "Widget"), kinds)).toBeUndefined();
    expect(eventKind(event("v1", "Pod", ""), kinds)).toBeUndefined();
  });
});
