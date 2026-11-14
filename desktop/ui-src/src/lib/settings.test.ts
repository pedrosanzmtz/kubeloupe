import { describe, expect, it } from "vitest";
import {
  applyPatch,
  clampWidth,
  contextToRestore,
  defaultSettings,
  DEFAULT_DETAIL_WIDTH,
  MIN_DETAIL_WIDTH,
  namespacesToRestore,
  sortOf,
  themeAttribute,
  toggleHidden,
  withSort,
} from "./settings";

const ctx = (name: string, current = false) => ({ name, current, namespace: null });

describe("applyPatch", () => {
  it("changes only the fields given", () => {
    const s = applyPatch(defaultSettings(), { theme: "light", detail_width: 600 });
    expect(s.theme).toBe("light");
    expect(s.detail_width).toBe(600);
    expect(s.logs).toEqual(defaultSettings().logs);
  });

  it("replaces only the map entries given", () => {
    let s = applyPatch(defaultSettings(), {
      kinds: { a: { sort_key: "name", sort_desc: false, hidden: [] }, b: { sort_key: "age", sort_desc: true, hidden: [] } },
    });
    s = applyPatch(s, { kinds: { a: { sort_key: "status", sort_desc: false, hidden: ["node"] } } });
    expect(s.kinds.a).toEqual({ sort_key: "status", sort_desc: false, hidden: ["node"] });
    expect(s.kinds.b.sort_key).toBe("age");
  });

  it("does not change the settings it was given", () => {
    const before = defaultSettings();
    applyPatch(before, { contexts: { x: { view: "v1/pods", namespaces: [], typed_namespaces: [] } } });
    expect(before.contexts).toEqual({});
  });
});

describe("contextToRestore", () => {
  it("restores a remembered context that still exists", () => {
    expect(contextToRestore("prod", [ctx("dev", true), ctx("prod")])).toBe("prod");
  });
  it("falls back when it is gone, already current or never set", () => {
    expect(contextToRestore("old", [ctx("dev", true)])).toBeNull();
    expect(contextToRestore("dev", [ctx("dev", true)])).toBeNull();
    expect(contextToRestore("", [ctx("dev", true)])).toBeNull();
  });
});

describe("namespacesToRestore", () => {
  it("drops namespaces that no longer exist", () => {
    expect(namespacesToRestore(["demo", "gone"], ["demo", "default"], false)).toEqual(["demo"]);
  });
  it("uses the default scope when none is left or none was remembered", () => {
    expect(namespacesToRestore(["gone"], ["demo"], false)).toBeNull();
    expect(namespacesToRestore([], ["demo"], false)).toBeNull();
  });
  it("keeps typed namespaces when they cannot be listed", () => {
    expect(namespacesToRestore(["team-a"], [], true)).toEqual(["team-a"]);
  });
});

describe("sort", () => {
  it("round-trips through the kind settings", () => {
    const k = withSort(undefined, { key: "age", dir: "desc" });
    expect(k).toEqual({ sort_key: "age", sort_desc: true, hidden: [] });
    expect(sortOf(k)).toEqual({ key: "age", dir: "desc" });
    expect(sortOf(withSort(k, null))).toBeNull();
    expect(withSort({ ...k, hidden: ["x"] }, null).hidden).toEqual(["x"]);
  });
});

describe("toggleHidden", () => {
  it("hides and shows a column, never the name", () => {
    expect(toggleHidden([], "node")).toEqual(["node"]);
    expect(toggleHidden(["node", "ip"], "node")).toEqual(["ip"]);
    expect(toggleHidden([], "name")).toEqual([]);
  });
});

describe("clampWidth", () => {
  it("keeps the width between the minimum and 75% of the window", () => {
    expect(clampWidth(100, 1600)).toBe(MIN_DETAIL_WIDTH);
    expect(clampWidth(5000, 1600)).toBe(1200);
    expect(clampWidth(500.4, 1600)).toBe(500);
  });
  it("uses the default for a missing width", () => {
    expect(clampWidth(0, 1600)).toBe(DEFAULT_DETAIL_WIDTH);
    expect(clampWidth(Number.NaN, 1600)).toBe(DEFAULT_DETAIL_WIDTH);
  });
});

it("themeAttribute: System leaves it to the media query", () => {
  expect(themeAttribute("system")).toBeNull();
  expect(themeAttribute("light")).toBe("light");
  expect(themeAttribute("dark")).toBe("dark");
});
