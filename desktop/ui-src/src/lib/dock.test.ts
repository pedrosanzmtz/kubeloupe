import { describe, expect, it } from "vitest";
import type { ResourceRef } from "./bindings";
import { DOCK_MIN_HEIGHT, clampHeight, closeTab, dockMenuFor, openTab, tabId } from "./dock";
import type { DockTab, DockTabSpec } from "./dock";

const POD: ResourceRef = { group: "", version: "v1", kind: "Pod", plural: "pods" } as ResourceRef;
const DEPLOY: ResourceRef = { group: "apps", version: "v1", kind: "Deployment", plural: "deployments" } as ResourceRef;
const CM: ResourceRef = { group: "", version: "v1", kind: "ConfigMap", plural: "configmaps" } as ResourceRef;

const logs = (name: string): DockTabSpec => ({ type: "podLogs", pod: { resource: POD, namespace: "demo", name } });
const shell = (name: string): DockTabSpec => ({ type: "terminal", pod: { resource: POD, namespace: "demo", name } });

describe("openTab", () => {
  it("appends a new tab and makes it active", () => {
    const a = openTab([], logs("web-1"));
    const b = openTab(a.tabs, logs("web-2"));
    expect(b.tabs.map((t) => t.title)).toEqual(["Logs: web-1", "Logs: web-2"]);
    expect(b.active).toBe(tabId(logs("web-2")));
  });

  it("focuses an open tab instead of adding a second one", () => {
    const a = openTab([], logs("web-1"));
    const b = openTab(a.tabs, shell("web-1"));
    const c = openTab(b.tabs, logs("web-1"));
    expect(c.tabs).toBe(b.tabs);
    expect(c.active).toBe(tabId(logs("web-1")));
  });

  it("keeps logs, shells and workload logs of the same name apart", () => {
    const ids = new Set([
      tabId(logs("web")),
      tabId(shell("web")),
      tabId({ type: "workloadLogs", workload: { kind: "Deployment", namespace: "demo", name: "web" } }),
      tabId({ type: "workloadLogs", workload: { kind: "StatefulSet", namespace: "demo", name: "web" } }),
    ]);
    expect(ids.size).toBe(4);
  });
});

describe("closeTab", () => {
  const tabs = ["a", "b", "c"].reduce((acc, n) => openTab(acc, logs(n)).tabs, [] as DockTab[]);
  const id = (n: string) => tabId(logs(n));

  it("keeps the active tab when another one closes", () => {
    const r = closeTab(tabs, id("a"), id("b"));
    expect(r.tabs.map((t) => t.id)).toEqual([id("a"), id("c")]);
    expect(r.active).toBe(id("a"));
  });

  it("moves to the right neighbour, or the left one at the end", () => {
    expect(closeTab(tabs, id("b"), id("b")).active).toBe(id("c"));
    expect(closeTab(tabs, id("c"), id("c")).active).toBe(id("b"));
  });

  it("leaves nothing active after the last tab", () => {
    const one = openTab([], logs("a")).tabs;
    expect(closeTab(one, id("a"), id("a"))).toEqual({ tabs: [], active: null });
  });

  it("ignores an unknown id", () => {
    expect(closeTab(tabs, id("a"), "nope").tabs).toBe(tabs);
  });
});

describe("dockMenuFor", () => {
  it("offers logs and a shell for pods, logs for workloads, nothing otherwise", () => {
    expect(dockMenuFor(POD, "demo", "web-1").map((m) => m.label)).toEqual(["Logs", "Shell"]);
    expect(dockMenuFor(DEPLOY, "demo", "web")).toEqual([
      { label: "Logs", spec: { type: "workloadLogs", workload: { kind: "Deployment", namespace: "demo", name: "web" } } },
    ]);
    expect(dockMenuFor(CM, "demo", "cfg")).toEqual([]);
  });
});

describe("clampHeight", () => {
  it("stays between the minimum and the room available", () => {
    expect(clampHeight(10, 800)).toBe(DOCK_MIN_HEIGHT);
    expect(clampHeight(400.4, 800)).toBe(400);
    expect(clampHeight(2000, 800)).toBe(800);
    // A tiny window still gets the minimum.
    expect(clampHeight(300, 50)).toBe(DOCK_MIN_HEIGHT);
  });
});
