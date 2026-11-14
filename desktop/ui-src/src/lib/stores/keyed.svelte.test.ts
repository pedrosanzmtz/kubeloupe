import { describe, expect, it } from "vitest";
import { flushSync } from "svelte";
import { KeyedStore } from "./keyed.svelte";

interface R {
  key: string;
  value: string;
}

const r = (key: string, value = key): R => ({ key, value });
const store = () => new KeyedStore<R>((row) => row.key);
const keys = (s: KeyedStore<R>) => s.entries.map((e) => e.key);

describe("KeyedStore", () => {
  it("replaces everything on a snapshot", () => {
    const s = store();
    s.applyBatch({ snapshot: true, applied: [r("a"), r("b")], deleted: [] });
    s.applyBatch({ snapshot: true, applied: [r("b", "b2"), r("c")], deleted: [] });
    expect(keys(s)).toEqual(["b", "c"]);
    expect(s.entries[0].row.value).toBe("b2");
    expect(s.size).toBe(2);
  });

  it("merges incremental batches: adds, updates in place, deletes", () => {
    const s = store();
    s.applyBatch({ snapshot: true, applied: [r("a"), r("b"), r("c")], deleted: [] });
    s.applyBatch({ snapshot: false, applied: [r("b", "new"), r("d")], deleted: ["a"] });
    expect(keys(s)).toEqual(["b", "c", "d"]);
    expect(s.entries.find((e) => e.key === "b")!.row.value).toBe("new");
  });

  it("ignores deletes of unknown keys", () => {
    const s = store();
    s.applyBatch({ snapshot: true, applied: [r("a")], deleted: [] });
    s.applyBatch({ snapshot: false, applied: [], deleted: ["zzz"] });
    expect(keys(s)).toEqual(["a"]);
  });

  it("applies deletes after the rows of the same batch", () => {
    const s = store();
    s.applyBatch({ snapshot: false, applied: [r("a")], deleted: ["a"] });
    expect(keys(s)).toEqual([]);
  });

  it("precomputes a lowercase search haystack over every field", () => {
    const s = store();
    s.applyBatch({ snapshot: true, applied: [r("Web-1", "RUNNING")], deleted: [] });
    expect(s.entries[0].hay).toContain("web-1");
    expect(s.entries[0].hay).toContain("running");
  });

  it("publishes a new array per batch and never mutates the old one", () => {
    const s = store();
    s.applyBatch({ snapshot: true, applied: [r("a")], deleted: [] });
    const before = s.entries;
    const unchanged = s.entries[0];
    s.applyBatch({ snapshot: false, applied: [r("b")], deleted: [] });
    expect(before).toHaveLength(1);
    expect(s.entries).not.toBe(before);
    // Rows a batch did not touch keep their entry object.
    expect(s.entries[0]).toBe(unchanged);
  });

  it("is reactive: an effect sees every batch", () => {
    const s = store();
    const seen: number[] = [];
    const cleanup = $effect.root(() => {
      $effect(() => {
        seen.push(s.entries.length);
      });
    });
    flushSync();
    s.applyBatch({ snapshot: true, applied: [r("a"), r("b")], deleted: [] });
    flushSync();
    s.clear();
    flushSync();
    cleanup();
    expect(seen).toEqual([0, 2, 0]);
  });
});
