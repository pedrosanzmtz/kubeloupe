import { describe, expect, it } from "vitest";
import type { ContainerDetail, ResourceRef } from "./bindings";
import { debugTargets, leftoverText, validImage } from "./debug";
import { dockMenuFor, openTab, tabId, tabTitle } from "./dock";
import type { DockTabSpec } from "./dock";

const c = (name: string, kind: ContainerDetail["kind"]) => ({ name, kind }) as ContainerDetail;
const POD = { group: "", version: "v1", kind: "Pod", plural: "pods" } as ResourceRef;

describe("debugTargets", () => {
  it("offers only regular containers", () => {
    const all = [c("init", "init"), c("app", "regular"), c("side", "regular"), c("debugger-x", "ephemeral")];
    expect(debugTargets(all)).toEqual(["app", "side"]);
  });
});

describe("validImage", () => {
  it("accepts references and the empty default, not spaces", () => {
    expect(validImage("nicolaka/netshoot")).toBe(true);
    expect(validImage("  busybox:1.37 ")).toBe(true);
    expect(validImage("")).toBe(true);
    expect(validImage("busy box")).toBe(false);
  });
});

describe("debug and node shell tabs", () => {
  const debug = (target: string): DockTabSpec => ({
    type: "debug",
    pod: { resource: POD, namespace: "demo", name: "web-1" },
    target,
    image: "busybox:1.37",
  });

  it("gets one tab per pod and target, apart from the plain shell", () => {
    const shell: DockTabSpec = { type: "terminal", pod: { resource: POD, namespace: "demo", name: "web-1" } };
    expect(new Set([tabId(debug("app")), tabId(debug("side")), tabId(shell)]).size).toBe(3);
    const a = openTab([], debug("app"));
    expect(openTab(a.tabs, debug("app")).tabs).toBe(a.tabs);
    expect(tabTitle(debug("app"))).toBe("Debug: web-1");
  });

  it("names node shells after the node", () => {
    const spec: DockTabSpec = { type: "nodeShell", node: "worker", namespace: "kube-system" };
    expect(tabId(spec)).toBe("node:worker");
    expect(tabTitle(spec)).toBe("Node: worker");
  });

  it("leaves the pod's dock menu alone (Debug is a registry action)", () => {
    expect(dockMenuFor(POD, "demo", "web-1").map((d) => d.label)).toEqual(["Logs", "Shell"]);
  });
});

describe("leftoverText", () => {
  it("counts the pods", () => {
    expect(leftoverText(1)).toMatch(/^A node shell pod/);
    expect(leftoverText(3)).toMatch(/^3 node shell pods/);
  });
});
