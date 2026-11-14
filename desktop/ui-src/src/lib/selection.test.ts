import { describe, expect, it } from "vitest";
import { rangeKeys } from "./selection";

const ORDER = ["a", "b", "c", "d", "e"];

describe("rangeKeys (shift-click)", () => {
  it("selects from the anchor down", () => {
    expect(rangeKeys(ORDER, "b", "d")).toEqual(["b", "c", "d"]);
  });
  it("selects from the anchor up", () => {
    expect(rangeKeys(ORDER, "d", "b")).toEqual(["b", "c", "d"]);
  });
  it("is just the row when anchor and target are the same", () => {
    expect(rangeKeys(ORDER, "c", "c")).toEqual(["c"]);
  });
  it("falls back to the clicked row without an anchor", () => {
    expect(rangeKeys(ORDER, null, "c")).toEqual(["c"]);
  });
  it("falls back to the clicked row when the anchor was filtered out", () => {
    expect(rangeKeys(ORDER, "zzz", "c")).toEqual(["c"]);
  });
  it("is empty when the clicked row is not visible", () => {
    expect(rangeKeys(ORDER, "a", "zzz")).toEqual([]);
  });
});
