import { describe, expect, it } from "vitest";
import type { ActionDef, ActionId } from "./bindings";
import {
  confirmCopy,
  drainLine,
  isDestructive,
  listTargets,
  needsDialog,
  replicasFromRow,
  sameResource,
  targetLabel,
} from "./actions";

function action(id: ActionId, extra: Partial<ActionDef> = {}): ActionDef {
  return { id, label: id, destructive: false, type_to_confirm: false, input: "none", ...extra };
}

const one = [{ namespace: "demo", name: "web" }];
const many = (n: number) => Array.from({ length: n }, (_, i) => ({ namespace: "demo", name: `p${i}` }));

describe("targets", () => {
  it("labels namespaced and cluster-scoped objects", () => {
    expect(targetLabel({ namespace: "demo", name: "web" })).toBe("demo/web");
    expect(targetLabel({ namespace: "", name: "node-1" })).toBe("node-1");
  });

  it("lists up to 8 names, then says how many more", () => {
    expect(listTargets(many(8))).toHaveLength(8);
    const listed = listTargets(many(11));
    expect(listed).toHaveLength(9);
    expect(listed[0]).toBe("demo/p0");
    expect(listed[8]).toBe("…and 3 more");
  });
});

describe("needsDialog / isDestructive", () => {
  it("runs a harmless single action right away", () => {
    expect(needsDialog(action("restart"), one)).toBe(false);
  });
  it("asks for destructive actions, actions with input and bulk actions", () => {
    expect(needsDialog(action("delete", { destructive: true }), one)).toBe(true);
    expect(needsDialog(action("scale", { input: "replicas" }), one)).toBe(true);
    expect(needsDialog(action("restart"), many(2))).toBe(true);
  });
  it("treats scaling to 0 as destructive", () => {
    expect(isDestructive(action("scale"), 0)).toBe(true);
    expect(isDestructive(action("scale"), 3)).toBe(false);
    expect(isDestructive(action("delete", { destructive: true }), 3)).toBe(true);
  });
});

describe("confirmCopy", () => {
  it("names the kind and count of a delete", () => {
    const del = action("delete", { destructive: true });
    expect(confirmCopy(del, "Pod", 1, 1)).toMatchObject({ title: "Delete Pod", button: "Delete" });
    const bulk = confirmCopy(del, "Pod", 3, 1);
    expect(bulk.title).toBe("Delete 3 Pods");
    expect(bulk.lead).toBe("This permanently deletes 3 Pods:");
    expect(bulk.button).toBe("Delete 3");
  });

  it("warns about force delete and drain", () => {
    expect(confirmCopy(action("forceDelete"), "Pod", 1, 1).warning).toMatch(/stuck terminating/);
    expect(confirmCopy(action("drain"), "Node", 1, 1).warning).toMatch(/PodDisruptionBudget/);
  });

  it("warns when scaling to 0", () => {
    const scale = action("scale", { input: "replicas" });
    expect(confirmCopy(scale, "Deployment", 1, 3)).toMatchObject({ button: "Scale to 3", warning: "" });
    const zero = confirmCopy(scale, "Deployment", 1, 0);
    expect(zero.button).toBe("Scale to 0");
    expect(zero.warning).toMatch(/stops every pod/);
  });

  it("uses the verb for the other actions", () => {
    expect(confirmCopy(action("cordon"), "Node", 2, 1)).toEqual({
      title: "Cordon 2 Nodes",
      lead: "Cordon:",
      warning: "",
      button: "Cordon",
    });
  });
});

describe("replicasFromRow", () => {
  const row = (ready: string) => ({ key: "k", name: "n", namespace: "ns", created: 0, cells: ["n", ready] });
  it("reads the desired count from the ready column", () => {
    expect(replicasFromRow(row("2/5"), ["name", "ready"])).toBe(5);
    expect(replicasFromRow(row("0/0"), ["name", "ready"])).toBe(0);
  });
  it("defaults to 1", () => {
    expect(replicasFromRow(row("?"), ["name", "ready"])).toBe(1);
    expect(replicasFromRow(row("2/5"), ["name"])).toBe(1);
    expect(replicasFromRow(undefined, ["name", "ready"])).toBe(1);
  });
});

describe("drainLine", () => {
  it("describes each drain event in words", () => {
    expect(drainLine({ type: "cordoned", node: "n1" })).toBe("Node n1 cordoned");
    expect(drainLine({ type: "planned", to_evict: 3, skipped: 2, unmanaged: 0 })).toBe("3 pods to evict, 2 skipped");
    expect(drainLine({ type: "planned", to_evict: 3, skipped: 2, unmanaged: 1 })).toMatch(
      /1 without a controller will not be recreated/,
    );
    expect(drainLine({ type: "evicted", namespace: "a", name: "b" })).toBe("Evicted a/b");
    expect(drainLine({ type: "waitingOnPdb", namespace: "a", name: "b", message: "" })).toMatch(/PodDisruptionBudget/);
    expect(drainLine({ type: "failed", namespace: "a", name: "b", error: "boom" })).toBe("a/b: failed. boom");
  });
});

describe("sameResource", () => {
  const pods = { group: "", version: "v1", kind: "Pod", plural: "pods", namespaced: true };
  it("compares group, version, kind and plural", () => {
    expect(sameResource(pods, { ...pods })).toBe(true);
    expect(sameResource(pods, { ...pods, version: "v2" })).toBe(false);
    expect(sameResource(pods, { ...pods, group: "x" })).toBe(false);
  });
});
