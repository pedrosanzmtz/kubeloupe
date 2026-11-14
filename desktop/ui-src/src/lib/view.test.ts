import { describe, expect, it } from "vitest";
import type { ColumnDef, ResourceKind, ResourceRow } from "./bindings";
import { columnsFor, EMPTY_METRICS } from "./columns";
import type { Ctx } from "./columns";
import type { Entry } from "./stores/keyed.svelte";
import { visibleEntries } from "./view";

const COLUMNS: ColumnDef[] = [
  { key: "name", label: "Name", kind: "text" },
  { key: "restarts", label: "Restarts", kind: "number" },
  { key: "age", label: "Age", kind: "age" },
];

const KIND: ResourceKind = {
  resource: { group: "", version: "v1", kind: "Pod", plural: "pods", namespaced: true },
  id: "v1/pods",
  label: "Pods",
  category: "workloads",
  crd: false,
  verbs: [],
  short_names: [],
  columns: COLUMNS,
  actions: [],
};

function entry(namespace: string, name: string, restarts: string, created: number): Entry<ResourceRow> {
  const row = { key: `${namespace}/${name}`, name, namespace, created, cells: [name, restarts, ""] };
  return { key: row.key, row, hay: `${namespace}\0${name}\0${restarts}`.toLowerCase() };
}

const ENTRIES = [
  entry("prod", "web-b", "10", 300),
  entry("dev", "Web-A", "2", 100),
  entry("prod", "db", "0", 200),
];
const CTX: Ctx = { metrics: EMPTY_METRICS, now: 1_000_000 };
const cols = columnsFor(KIND);
const keys = (es: Entry<ResourceRow>[]) => es.map((e) => e.key);

describe("visibleEntries", () => {
  it("returns rows in key order without a sort", () => {
    expect(keys(visibleEntries(ENTRIES, "", null, cols, CTX))).toEqual(["dev/Web-A", "prod/db", "prod/web-b"]);
  });

  it("does not reorder the input array", () => {
    const input = ENTRIES.slice();
    visibleEntries(input, "", null, cols, CTX);
    expect(input).toEqual(ENTRIES);
  });

  it("filters case-insensitively and ignores surrounding blanks", () => {
    expect(keys(visibleEntries(ENTRIES, "  WEB ", null, cols, CTX))).toEqual(["dev/Web-A", "prod/web-b"]);
    expect(keys(visibleEntries(ENTRIES, "nothing", null, cols, CTX))).toEqual([]);
  });

  it("sorts numbers numerically, both directions", () => {
    const asc = visibleEntries(ENTRIES, "", { key: "restarts", dir: "asc" }, cols, CTX);
    expect(keys(asc)).toEqual(["prod/db", "dev/Web-A", "prod/web-b"]);
    const desc = visibleEntries(ENTRIES, "", { key: "restarts", dir: "desc" }, cols, CTX);
    expect(keys(desc)).toEqual(["prod/web-b", "dev/Web-A", "prod/db"]);
  });

  it("sorts by creation time for the age column", () => {
    const sorted = visibleEntries(ENTRIES, "", { key: "age", dir: "asc" }, cols, CTX);
    expect(keys(sorted)).toEqual(["dev/Web-A", "prod/db", "prod/web-b"]);
  });

  it("breaks ties by key", () => {
    const tied = [entry("b", "x", "1", 0), entry("a", "y", "1", 0), entry("c", "z", "1", 0)];
    const desc = visibleEntries(tied, "", { key: "restarts", dir: "desc" }, cols, CTX);
    expect(keys(desc)).toEqual(["a/y", "b/x", "c/z"]);
  });

  it("falls back to key order for an unknown sort column", () => {
    const sorted = visibleEntries(ENTRIES, "", { key: "gone", dir: "desc" }, cols, CTX);
    expect(keys(sorted)).toEqual(["dev/Web-A", "prod/db", "prod/web-b"]);
  });

  it("filters before sorting", () => {
    const sorted = visibleEntries(ENTRIES, "prod", { key: "restarts", dir: "desc" }, cols, CTX);
    expect(keys(sorted)).toEqual(["prod/web-b", "prod/db"]);
  });
});
