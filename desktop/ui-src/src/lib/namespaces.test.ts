import { describe, expect, it } from "vitest";
import {
  combineWatches,
  filterNamespaces,
  joinNames,
  MAX_NAMESPACES,
  namespacesLabel,
  normalizeNamespaces,
  sameNamespaces,
  toggleNamespace,
} from "./namespaces";
import type { WatchStatus } from "./namespaces";

const many = (n: number) => Array.from({ length: n }, (_, i) => `ns-${i}`);

describe("namespace selection", () => {
  it("normalizes: trims, drops blanks and duplicates, caps", () => {
    expect(normalizeNamespaces([" demo ", "", "default", "demo"])).toEqual(["demo", "default"]);
    expect(normalizeNamespaces(many(15))).toHaveLength(MAX_NAMESPACES);
  });

  it("toggles a namespace in and out, never past the cap", () => {
    expect(toggleNamespace([], "demo")).toEqual(["demo"]);
    expect(toggleNamespace(["demo", "default"], "demo")).toEqual(["default"]);
    expect(toggleNamespace(many(MAX_NAMESPACES), "extra")).toEqual(many(MAX_NAMESPACES));
    expect(toggleNamespace(many(MAX_NAMESPACES), "ns-0")).toHaveLength(MAX_NAMESPACES - 1);
  });

  it("labels, compares, filters and joins", () => {
    expect(namespacesLabel([])).toBe("All namespaces");
    expect(namespacesLabel(["demo"])).toBe("demo");
    expect(namespacesLabel(["demo", "default"])).toBe("2 namespaces");
    expect(sameNamespaces(["a", "b"], ["a", "b"])).toBe(true);
    expect(sameNamespaces(["a", "b"], ["b", "a"])).toBe(false);
    expect(filterNamespaces(["demo", "default", "kube-system"], " DE ")).toEqual(["demo", "default"]);
    expect(filterNamespaces(["a"], "")).toEqual(["a"]);
    expect(joinNames(["a"])).toBe("a");
    expect(joinNames(["a", "b", "c"])).toBe("a, b and c");
  });
});

describe("combineWatches", () => {
  const w = (namespace: string | null, state: WatchStatus["state"], error = "", status?: string): WatchStatus => ({
    namespace,
    state,
    error,
    status,
  });

  it("keeps the single-watch messages", () => {
    expect(combineWatches([w(null, "live")], "Pods", true)).toEqual({ ok: true, text: "Live", error: "" });
    expect(combineWatches([w(null, "reconnecting", "reset")], "Pods", true)).toEqual({
      ok: false,
      text: "Reconnecting…",
      error: "Watch interrupted, retrying: reset",
    });
    expect(combineWatches([w(null, "forbidden", "403")], "Pods", true).error).toBe(
      "You can't list Pods across all namespaces. Pick a namespace you have access to. (403)",
    );
    expect(combineWatches([w("team-b", "forbidden", "403")], "Pods", true).error).toBe(
      "You can't list Pods in namespace team-b. (403)",
    );
    expect(combineWatches([w(null, "connecting")], "Pods", true)).toEqual({ ok: null, text: "Connecting…", error: null });
  });

  it("is Live only when every watch is live", () => {
    expect(combineWatches([w("a", "live"), w("b", "live")], "Pods", true)).toEqual({ ok: true, text: "Live", error: "" });
    expect(combineWatches([w("a", "live"), w("b", "connecting")], "Pods", true).text).toBe("Connecting…");
  });

  it("names the namespaces that are reconnecting or not allowed", () => {
    const r = combineWatches([w("a", "live"), w("b", "reconnecting", "reset")], "Pods", true);
    expect(r).toEqual({ ok: false, text: "Reconnecting… (b)", error: "Watch of b interrupted, retrying: reset" });

    const f = combineWatches([w("a", "live"), w("b", "forbidden", "403")], "Pods", true);
    expect(f).toEqual({ ok: false, text: "Not allowed in b", error: "You can't list Pods in namespace b. (403)" });

    const all = combineWatches([w("a", "forbidden", "x"), w("b", "forbidden", "y")], "Pods", true);
    expect(all.text).toBe("Not allowed");
    expect(all.error).toContain("namespace a");
    expect(all.error).toContain("namespace b");

    const failed = combineWatches([w("a", "failed", "refused", "Cluster unreachable"), w("b", "live")], "Pods", true);
    expect(failed).toEqual({ ok: false, text: "Cluster unreachable (a)", error: "a: refused" });
  });
});
