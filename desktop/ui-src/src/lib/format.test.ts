import { describe, expect, it } from "vitest";
import { formatAge, formatCpu, formatMemory } from "./format";

describe("formatCpu", () => {
  it("uses millicores below one core", () => {
    expect(formatCpu(0)).toBe("0m");
    expect(formatCpu(250)).toBe("250m");
    expect(formatCpu(999)).toBe("999m");
  });
  it("uses cores from one core, without trailing zeros", () => {
    expect(formatCpu(1000)).toBe("1");
    expect(formatCpu(1500)).toBe("1.5");
    expect(formatCpu(2250)).toBe("2.25");
    expect(formatCpu(4001)).toBe("4");
  });
});

describe("formatMemory", () => {
  it("keeps bytes below 1Ki", () => {
    expect(formatMemory(0)).toBe("0B");
    expect(formatMemory(1023)).toBe("1023B");
  });
  it("uses binary units without trailing zeros", () => {
    expect(formatMemory(1024)).toBe("1Ki");
    expect(formatMemory(128 * 1024 * 1024)).toBe("128Mi");
    expect(formatMemory(1.5 * 1024 ** 3)).toBe("1.5Gi");
    expect(formatMemory(2 * 1024 ** 4)).toBe("2Ti");
  });
  it("stops at Pi", () => {
    expect(formatMemory(2048 * 1024 ** 5)).toBe("2048Pi");
  });
});

describe("formatAge", () => {
  const now = 1_000_000_000; // ms
  const created = (secsAgo: number) => now / 1000 - secsAgo;
  it("falls back to the backend's text without a creation time", () => {
    expect(formatAge(0, now, "<unknown>")).toBe("<unknown>");
    expect(formatAge(0, now, "")).toBe("");
  });
  it("picks the largest whole unit", () => {
    expect(formatAge(created(5), now, "")).toBe("5s");
    expect(formatAge(created(59), now, "")).toBe("59s");
    expect(formatAge(created(60), now, "")).toBe("1m");
    expect(formatAge(created(3599), now, "")).toBe("59m");
    expect(formatAge(created(3600), now, "")).toBe("1h");
    expect(formatAge(created(86399), now, "")).toBe("23h");
    expect(formatAge(created(86400 * 3), now, "")).toBe("3d");
  });
  it("never goes negative for clock skew", () => {
    expect(formatAge(created(-30), now, "")).toBe("0s");
  });
});
