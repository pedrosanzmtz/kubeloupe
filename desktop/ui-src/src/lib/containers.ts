import type { ContainerDetail, StateInfo } from "./bindings";

/** How a container state is coloured: red for failures, amber for not yet. */
export type Tone = "ok" | "warn" | "bad" | "muted";

/** Waiting reasons that mean something is broken, not just starting. */
const BAD_WAITING = new Set([
  "CrashLoopBackOff",
  "ImagePullBackOff",
  "ErrImagePull",
  "InvalidImageName",
  "CreateContainerConfigError",
  "CreateContainerError",
  "RunContainerError",
  "PreStartHookError",
  "PostStartHookError",
]);

export function stateTone(s: StateInfo): Tone {
  switch (s.state) {
    case "running":
      return "ok";
    case "waiting":
      return BAD_WAITING.has(s.reason) ? "bad" : "warn";
    case "terminated":
      return s.exit_code === 0 && s.reason !== "OOMKilled" ? "muted" : "bad";
    default:
      return "muted";
  }
}

const capital = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

/** `Waiting: CrashLoopBackOff`, `Terminated: OOMKilled (exit 137)`, `Running`. */
export function stateText(s: StateInfo): string {
  if (!s.state) return "No status yet";
  let text = capital(s.state);
  if (s.reason) text += `: ${s.reason}`;
  if (s.state === "terminated") text += ` (exit ${s.exit_code})`;
  return text;
}

/** `5m ago` / `started 5m ago`, or empty. */
export function stateSince(s: StateInfo): string {
  if (!s.since) return "";
  return s.state === "running" ? `started ${s.since} ago` : `${s.since} ago`;
}

/** `cpu 100m / 500m · memory 64Mi / —`: request / limit per resource. */
export function resourcesText(c: Pick<ContainerDetail, "requests" | "limits">): string {
  const names = [...new Set([...Object.keys(c.requests), ...Object.keys(c.limits)])].sort();
  return names
    .map((n) => `${n} ${c.requests[n] ?? "—"} / ${c.limits[n] ?? "—"}`)
    .join(" · ");
}

/** `secret:db/password` → a readable hint; literal values show nothing. */
export function envSourceText(source: string): string {
  return source ? `from ${source}` : "value set";
}
