import { describe, expect, it } from "vitest";
import type { HelmChartInfo, HelmReleaseRow } from "./bindings";
import {
  ageOf,
  chartKeys,
  chartLabel,
  chartsByRelease,
  defaultDiffPair,
  releaseKey,
  rollbackTargets,
  statusClass,
  visibleReleases,
} from "./helm";

const row = (namespace: string, name: string, status = "deployed", revision = 1): HelmReleaseRow => ({
  name,
  namespace,
  revision,
  status,
  updated: 0,
  storage: "secret",
  history: [{ revision, status, updated: 0 }],
});

const chart = (namespace: string, name: string, c: string): HelmChartInfo => ({
  namespace,
  name,
  revision: 1,
  chart: c,
  chart_version: "1.0.0",
  app_version: "2.0",
  error: "",
});

describe("helm helpers", () => {
  it("labels charts like helm list", () => {
    expect(chartLabel("nginx", "1.2.3")).toBe("nginx-1.2.3");
    expect(chartLabel("nginx", "")).toBe("nginx");
    expect(chartLabel("", "1.2.3")).toBe("");
  });

  it("builds chart keys for the current revisions", () => {
    expect(chartKeys([row("a", "web", "deployed", 3)])).toEqual([
      { namespace: "a", name: "web", revision: 3, storage: "secret" },
    ]);
  });

  it("filters by name, namespace, status and chart, sorted", () => {
    const rows = [row("b", "web"), row("a", "db", "failed"), row("a", "cache")];
    const charts = chartsByRelease([chart("a", "cache", "redis")]);
    expect(visibleReleases(rows, charts, "").map(releaseKey)).toEqual(["a/cache", "a/db", "b/web"]);
    expect(visibleReleases(rows, charts, "REDIS").map(releaseKey)).toEqual(["a/cache"]);
    expect(visibleReleases(rows, charts, "failed").map(releaseKey)).toEqual(["a/db"]);
    expect(visibleReleases(rows, charts, " web ").map(releaseKey)).toEqual(["b/web"]);
  });

  it("colours statuses", () => {
    expect(statusClass("deployed")).toBe("phase-Running");
    expect(statusClass("failed")).toBe("phase-Failed");
    expect(statusClass("pending-upgrade")).toBe("phase-Pending");
    expect(statusClass("superseded")).toBe("");
    expect(statusClass("")).toBe("");
  });

  it("picks the revisions to diff", () => {
    const h = [{ revision: 5 }, { revision: 3 }, { revision: 1 }];
    expect(defaultDiffPair(h, 5)).toEqual([3, 5]);
    expect(defaultDiffPair(h, 3)).toEqual([1, 3]);
    expect(defaultDiffPair(h, 1)).toEqual([1, 3]);
    expect(defaultDiffPair([{ revision: 1 }], 1)).toBeNull();
  });

  it("offers every other revision for a rollback", () => {
    expect(rollbackTargets([{ revision: 3 }, { revision: 2 }, { revision: 1 }], 3)).toEqual([2, 1]);
    expect(rollbackTargets([{ revision: 1 }], 1)).toEqual([]);
  });

  it("formats ages", () => {
    const now = 1_000_000 * 1000;
    expect(ageOf(0, now)).toBe("");
    expect(ageOf(1_000_000 - 30, now)).toBe("30s");
    expect(ageOf(1_000_000 - 600, now)).toBe("10m");
    expect(ageOf(1_000_000 - 7200, now)).toBe("2h");
    expect(ageOf(1_000_000 - 3 * 86400, now)).toBe("3d");
  });
});
