import { describe, expect, it } from "vitest";
import type { RelatedGroup } from "./bindings";
import { cellClass, isObjectGroup, itemKey, summaryLine } from "./related";

const group = (kind: string, api_version: string): RelatedGroup => ({
  title: "t",
  api_version,
  kind,
  columns: ["Name"],
  name_column: 0,
  items: [],
  error: "",
});

describe("related", () => {
  it("joins the summary, skipping empty parts", () => {
    expect(summaryLine(["3/3 ready", "", "RollingUpdate 25%/25%", "revision 4"])).toBe(
      "3/3 ready · RollingUpdate 25%/25% · revision 4",
    );
    expect(summaryLine([])).toBe("");
  });

  it("colours statuses and short ready counts", () => {
    expect(cellClass("Status", "Running")).toBe("phase-Running");
    expect(cellClass("Ready", "1/3")).toBe("short");
    expect(cellClass("Ready", "3/3")).toBe("");
    expect(cellClass("Name", "Running")).toBe("");
  });

  it("tells object groups from plain rows", () => {
    expect(isObjectGroup(group("Pod", "v1"))).toBe(true);
    expect(isObjectGroup(group("", ""))).toBe(false);
    const item = { namespace: "demo", name: "web-1", cells: [] };
    expect(itemKey(group("Pod", "v1"), item, 2)).toBe("demo/web-1/2");
    expect(itemKey(group("", ""), item, 2)).toBe("2");
  });
});
