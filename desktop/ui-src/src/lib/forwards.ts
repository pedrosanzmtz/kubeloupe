import type { ForwardInfo, ForwardKind, ForwardTarget, ResourceRef } from "./bindings";

/** The sidebar view that lists the running forwards. */
export const PORT_FORWARDS_VIEW = "port-forwards";

const KINDS: Record<string, ForwardKind> = {
  "/Pod": "pod",
  "/Service": "service",
  "apps/Deployment": "deployment",
  "apps/StatefulSet": "statefulSet",
  "apps/ReplicaSet": "replicaSet",
  "apps/DaemonSet": "daemonSet",
};

const SHORT: Record<ForwardKind, string> = {
  pod: "pod",
  service: "svc",
  deployment: "deploy",
  statefulSet: "sts",
  replicaSet: "rs",
  daemonSet: "ds",
};

/** What a forward to this kind is, or null if it cannot be forwarded. */
export function forwardKindOf(r: ResourceRef): ForwardKind | null {
  return KINDS[`${r.group}/${r.kind}`] ?? null;
}

/** `svc/demo/api`. */
export function forwardLabel(t: ForwardTarget): string {
  return `${SHORT[t.kind]}/${t.namespace}/${t.name}`;
}

export function sameTarget(a: ForwardTarget, b: ForwardTarget): boolean {
  return a.kind === b.kind && a.namespace === b.namespace && a.name === b.name;
}

export type Parsed<T> = { ok: true; value: T } | { ok: false; error: string };

/** The local port field: empty is automatic (null), 0 is random. */
export function parseLocalPort(text: string): Parsed<number | null> {
  const s = text.trim();
  if (s === "") return { ok: true, value: null };
  if (!/^\d+$/.test(s) || Number(s) > 65535) {
    return { ok: false, error: "The local port must be a number between 0 and 65535, or empty." };
  }
  return { ok: true, value: Number(s) };
}

/** The remote port field: a port number or a port name (IANA_SVC_NAME). */
export function parseRemotePort(text: string): Parsed<string> {
  const s = text.trim();
  if (s === "") return { ok: false, error: "Enter the remote port: a number or a port name." };
  if (/^\d+$/.test(s)) {
    const n = Number(s);
    return n >= 1 && n <= 65535
      ? { ok: true, value: String(n) }
      : { ok: false, error: "The remote port must be between 1 and 65535." };
  }
  const name = s.length <= 15 && /^[a-z0-9]([a-z0-9-]*[a-z0-9])?$/.test(s) && /[a-z]/.test(s) && !s.includes("--");
  return name ? { ok: true, value: s } : { ok: false, error: `"${s}" is neither a port number nor a port name.` };
}

/** Only forwarded URLs on this machine may be opened (the capability says the same). */
export function isLocalUrl(url: string): boolean {
  return /^http:\/\/(127\.0\.0\.1|localhost):\d{1,5}(\/.*)?$/.test(url);
}

export interface ForwardState {
  text: string;
  tone: "ok" | "error";
}

/** What the status column says. */
export function forwardState(f: ForwardInfo): ForwardState {
  const s = f.status;
  if (s.last_error) return { text: s.last_error, tone: "error" };
  const open = s.connections === 1 ? "1 connection" : `${s.connections} connections`;
  return { text: s.connections > 0 ? `Active, ${open}` : "Listening", tone: "ok" };
}

/** "8080 → 80", or "8080 → http (80)" for a named port. */
export function portsLabel(f: ForwardInfo): string {
  const remote = f.remote === String(f.remote_port) ? f.remote : `${f.remote} (${f.remote_port})`;
  return `${f.local_port} → ${remote}`;
}
