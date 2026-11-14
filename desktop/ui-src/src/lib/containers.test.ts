import { describe, expect, it } from "vitest";
import type { StateInfo } from "./bindings";
import { envSourceText, resourcesText, stateSince, stateText, stateTone } from "./containers";

const st = (s: Partial<StateInfo>): StateInfo => ({
  state: "",
  reason: "",
  message: "",
  exit_code: 0,
  since: "",
  ...s,
});

describe("stateTone", () => {
  it("is red for crash loops, pull errors and failed or OOM-killed runs", () => {
    expect(stateTone(st({ state: "waiting", reason: "CrashLoopBackOff" }))).toBe("bad");
    expect(stateTone(st({ state: "waiting", reason: "ImagePullBackOff" }))).toBe("bad");
    expect(stateTone(st({ state: "terminated", reason: "Error", exit_code: 1 }))).toBe("bad");
    expect(stateTone(st({ state: "terminated", reason: "OOMKilled", exit_code: 137 }))).toBe("bad");
  });
  it("is amber while starting, green when running, muted when done", () => {
    expect(stateTone(st({ state: "waiting", reason: "ContainerCreating" }))).toBe("warn");
    expect(stateTone(st({ state: "running" }))).toBe("ok");
    expect(stateTone(st({ state: "terminated", reason: "Completed" }))).toBe("muted");
    expect(stateTone(st({}))).toBe("muted");
  });
});

describe("stateText", () => {
  it("names the state, the reason and the exit code", () => {
    expect(stateText(st({ state: "running" }))).toBe("Running");
    expect(stateText(st({ state: "waiting", reason: "CrashLoopBackOff" }))).toBe("Waiting: CrashLoopBackOff");
    expect(stateText(st({ state: "terminated", reason: "OOMKilled", exit_code: 137 }))).toBe(
      "Terminated: OOMKilled (exit 137)",
    );
    expect(stateText(st({}))).toBe("No status yet");
  });
  it("says when", () => {
    expect(stateSince(st({ state: "running", since: "5m" }))).toBe("started 5m ago");
    expect(stateSince(st({ state: "terminated", since: "2h" }))).toBe("2h ago");
    expect(stateSince(st({ state: "waiting" }))).toBe("");
  });
});

describe("resourcesText", () => {
  it("pairs requests and limits per resource", () => {
    expect(resourcesText({ requests: { cpu: "100m", memory: "64Mi" }, limits: { memory: "128Mi" } })).toBe(
      "cpu 100m / — · memory 64Mi / 128Mi",
    );
    expect(resourcesText({ requests: {}, limits: {} })).toBe("");
  });
});

describe("envSourceText", () => {
  it("shows references, never values", () => {
    expect(envSourceText("secret:db/password")).toBe("from secret:db/password");
    expect(envSourceText("")).toBe("value set");
  });
});
