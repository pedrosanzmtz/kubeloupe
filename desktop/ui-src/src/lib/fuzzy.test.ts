import { describe, expect, it } from "vitest";
import { bestScore, fuzzyScore } from "./fuzzy";

describe("fuzzyScore", () => {
  it("needs the characters in order", () => {
    expect(fuzzyScore("dpl", "deployments")).not.toBeNull();
    expect(fuzzyScore("ldp", "deployments")).toBeNull();
  });

  it("ranks exact over prefix over substring over scattered", () => {
    const exact = fuzzyScore("deploy", "deploy")!;
    const prefix = fuzzyScore("deploy", "deployments")!;
    const sub = fuzzyScore("ploy", "deployments")!;
    const scattered = fuzzyScore("dpls", "deployments")!;
    expect(exact).toBeGreaterThan(prefix);
    expect(prefix).toBeGreaterThan(sub);
    expect(sub).toBeGreaterThan(scattered);
  });

  it("prefers word starts", () => {
    expect(fuzzyScore("pc", "PriorityClasses")!).toBeGreaterThan(fuzzyScore("pc", "spaces")!);
  });

  it("ignores case and matches everything with an empty query", () => {
    expect(fuzzyScore("POD", "pods")).not.toBeNull();
    expect(fuzzyScore("", "anything")).toBe(0);
  });

  it("takes the best of several terms", () => {
    expect(bestScore("svc", ["Services", "services", "svc"])).toBe(1000);
    expect(bestScore("zzz", ["Services"])).toBeNull();
  });
});
