import { describe, expect, it } from "vitest";
import type { UsageReference } from "./bindings";
import { clockTime, frameFor, linePoints, niceMax, niceMaxBytes, refLines, sparkPoints, yOf } from "./usage";

const NO_REF: UsageReference = {
  cpu_request: 0,
  cpu_limit: 0,
  memory_request: 0,
  memory_limit: 0,
  cpu_allocatable: 0,
  memory_allocatable: 0,
};
const MI = 1024 * 1024;
const sample = (t: number, cpu: number, mem: number) => ({
  t,
  cpu_millicores: cpu,
  memory_bytes: mem,
});

describe("sparkPoints", () => {
  it("scales to the largest value, 0 at the bottom", () => {
    expect(sparkPoints([0, 5, 10], 20, 10)).toBe("0,10 10,5 20,0");
  });
  it("draws one value flat and nothing for none", () => {
    expect(sparkPoints([3], 20, 10)).toBe("0,0 20,0");
    expect(sparkPoints([], 20, 10)).toBe("");
  });
  it("keeps all zeros on the bottom and clamps to an explicit max", () => {
    expect(sparkPoints([0, 0], 10, 10)).toBe("0,10 10,10");
    expect(sparkPoints([5, 40], 10, 10, 20)).toBe("0,7.5 10,0");
  });
});

describe("axis tops", () => {
  it("rounds up to 1, 2 or 5 times a power of ten", () => {
    expect(niceMax(0)).toBe(1);
    expect(niceMax(7)).toBe(10);
    expect(niceMax(130)).toBe(200);
    expect(niceMax(250)).toBe(500);
    expect(niceMax(1000)).toBe(1000);
  });
  it("rounds memory up to powers of two of a binary unit", () => {
    expect(niceMaxBytes(0)).toBe(MI);
    expect(niceMaxBytes(100 * MI)).toBe(128 * MI);
    expect(niceMaxBytes(129 * MI)).toBe(256 * MI);
    expect(niceMaxBytes(3 * 1024 * MI)).toBe(4 * 1024 * MI);
    expect(niceMaxBytes(1000)).toBe(1024);
  });
});

describe("refLines", () => {
  it("keeps only what is set", () => {
    const ref = { ...NO_REF, cpu_request: 100, memory_limit: 128 * MI };
    expect(refLines(ref, "cpu")).toEqual([{ label: "request", value: 100 }]);
    expect(refLines(ref, "memory")).toEqual([{ label: "limit", value: 128 * MI }]);
    expect(refLines({ ...NO_REF, cpu_allocatable: 4000 }, "cpu")).toEqual([{ label: "allocatable", value: 4000 }]);
    expect(refLines(NO_REF, "memory")).toEqual([]);
  });
});

describe("chart frame", () => {
  it("fits the usage and the reference lines", () => {
    const lines = [
      {
        name: "total",
        samples: [sample(0, 50, MI), sample(120_000, 150, 2 * MI)],
      },
    ];
    const f = frameFor(lines, [{ label: "limit", value: 400 }], "cpu", 100, 50);
    expect(f).toEqual({ w: 100, h: 50, t0: 0, t1: 120_000, max: 500 });
    expect(linePoints(f, lines[0].samples, "cpu")).toBe("0,45 100,35");
    expect(yOf(f, 400)).toBe(10);
    expect(frameFor(lines, [], "memory", 100, 50).max).toBe(4 * MI);
  });
  it("spans at least a minute so the first samples do not fill the width", () => {
    const f = frameFor([{ name: "total", samples: [sample(600_000, 10, 1)] }], [], "cpu", 60, 10);
    expect(f.t0).toBe(540_000);
    expect(linePoints(f, [sample(600_000, 10, 1)], "cpu")).toBe("60,5");
    expect(frameFor([], [], "cpu", 60, 10)).toMatchObject({
      t0: -60_000,
      t1: 0,
      max: 1,
    });
  });
});

it("clockTime is local hours and minutes", () => {
  const d = new Date(2026, 0, 2, 9, 5);
  expect(clockTime(d.getTime())).toBe("09:05");
});
