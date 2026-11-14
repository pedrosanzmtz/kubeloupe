import { describe, expect, it } from "vitest";
import type { ForwardInfo, ResourceRef } from "./bindings";
import {
  forwardKindOf,
  forwardLabel,
  forwardState,
  isLocalUrl,
  parseLocalPort,
  parseRemotePort,
  portsLabel,
  sameTarget,
} from "./forwards";

const ref = (group: string, kind: string): ResourceRef => ({
  group,
  version: "v1",
  kind,
  plural: kind.toLowerCase() + "s",
  namespaced: true,
});

function info(over: Partial<ForwardInfo> = {}, status: Partial<ForwardInfo["status"]> = {}): ForwardInfo {
  return {
    id: "pf-1",
    target: { kind: "service", namespace: "demo", name: "api" },
    remote: "80",
    remote_port: 80,
    local_port: 40080,
    url: "http://127.0.0.1:40080",
    started_at: 0,
    ...over,
    status: { pod: "api-1", connections: 0, total_connections: 0, last_error: "", ...status },
  };
}

describe("forward targets", () => {
  it("maps kinds that can be forwarded", () => {
    expect(forwardKindOf(ref("", "Pod"))).toBe("pod");
    expect(forwardKindOf(ref("", "Service"))).toBe("service");
    expect(forwardKindOf(ref("apps", "Deployment"))).toBe("deployment");
    expect(forwardKindOf(ref("apps", "StatefulSet"))).toBe("statefulSet");
    expect(forwardKindOf(ref("", "ConfigMap"))).toBeNull();
    expect(forwardKindOf(ref("example.com", "Service"))).toBeNull();
  });

  it("labels and compares", () => {
    const t = { kind: "service" as const, namespace: "demo", name: "api" };
    expect(forwardLabel(t)).toBe("svc/demo/api");
    expect(forwardLabel({ ...t, kind: "deployment" })).toBe("deploy/demo/api");
    expect(sameTarget(t, { ...t })).toBe(true);
    expect(sameTarget(t, { ...t, kind: "pod" })).toBe(false);
  });
});

describe("port fields", () => {
  it("parses the local port", () => {
    expect(parseLocalPort("")).toEqual({ ok: true, value: null });
    expect(parseLocalPort(" 8080 ")).toEqual({ ok: true, value: 8080 });
    expect(parseLocalPort("0")).toEqual({ ok: true, value: 0 });
    expect(parseLocalPort("65536").ok).toBe(false);
    expect(parseLocalPort("abc").ok).toBe(false);
  });

  it("parses the remote port as a number or a name", () => {
    expect(parseRemotePort("80")).toEqual({ ok: true, value: "80" });
    expect(parseRemotePort("http")).toEqual({ ok: true, value: "http" });
    expect(parseRemotePort("http-alt")).toEqual({ ok: true, value: "http-alt" });
    for (const bad of ["", "0", "70000", "HTTP", "-a", "a--b", "averyveryverylongname"]) {
      expect(parseRemotePort(bad).ok, bad).toBe(false);
    }
  });
});

describe("urls and status", () => {
  it("only accepts local http URLs", () => {
    expect(isLocalUrl("http://127.0.0.1:8080")).toBe(true);
    expect(isLocalUrl("http://localhost:3000/path")).toBe(true);
    expect(isLocalUrl("https://127.0.0.1:8080")).toBe(false);
    expect(isLocalUrl("http://127.0.0.1.evil.com:80")).toBe(false);
    expect(isLocalUrl("http://example.com:80")).toBe(false);
  });

  it("describes the status", () => {
    expect(forwardState(info())).toEqual({ text: "Listening", tone: "ok" });
    expect(forwardState(info({}, { connections: 2 }))).toEqual({ text: "Active, 2 connections", tone: "ok" });
    expect(forwardState(info({}, { last_error: "no ready pod backs service demo/api" }))).toEqual({
      text: "no ready pod backs service demo/api",
      tone: "error",
    });
  });

  it("labels the ports", () => {
    expect(portsLabel(info())).toBe("40080 → 80");
    expect(portsLabel(info({ remote: "http" }))).toBe("40080 → http (80)");
  });
});
