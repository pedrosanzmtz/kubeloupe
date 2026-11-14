// Pure logic of the bottom dock: which tabs exist, which one is active.
// The reactive store is `stores/dock.svelte.ts`.

import type { LogContainer, ResourceRef } from "./bindings";
import { WORKLOAD_KINDS } from "./logs";
import type { WorkloadRef } from "./logs";

export interface PodTabRef {
  /** The Pod kind, to look up its containers. */
  resource: ResourceRef;
  namespace: string;
  name: string;
}

export type DockTabSpec =
  /** `containers`: the pod's, if known; otherwise the tab looks them up. */
  | { type: "podLogs"; pod: PodTabRef; containers?: LogContainer[] }
  | { type: "workloadLogs"; workload: WorkloadRef }
  /** `container`: undefined picks the pod's first regular container. */
  | { type: "terminal"; pod: PodTabRef; container?: string | null }
  /**
   * A shell on this machine, pointed at `context` and `namespace` as they
   * were when it opened. `seq` tells several of them apart.
   */
  | { type: "localShell"; seq: number; context: string; namespace: string };

export type DockTab = DockTabSpec & {
  /** Stable per object: opening the same thing again focuses this tab. */
  id: string;
  title: string;
};

export const DOCK_MIN_HEIGHT = 120;
export const DOCK_DEFAULT_HEIGHT = 300;

export function tabId(spec: DockTabSpec): string {
  switch (spec.type) {
    case "podLogs":
      return `logs:${spec.pod.namespace}/${spec.pod.name}`;
    case "workloadLogs":
      return `wlogs:${spec.workload.kind}:${spec.workload.namespace}/${spec.workload.name}`;
    case "terminal":
      return `shell:${spec.pod.namespace}/${spec.pod.name}`;
    case "localShell":
      return `local:${spec.seq}`;
  }
}

export function tabTitle(spec: DockTabSpec): string {
  switch (spec.type) {
    case "podLogs":
      return `Logs: ${spec.pod.name}`;
    case "workloadLogs":
      return `Logs: ${spec.workload.kind.toLowerCase()}/${spec.workload.name}`;
    case "terminal":
      return `Shell: ${spec.pod.name}`;
    case "localShell":
      return localShellTitle(spec.context, spec.namespace);
  }
}

/** `<context> · <namespace>`, or "Local shell" without a context. */
export function localShellTitle(context: string, namespace: string): string {
  return context ? `${context} · ${namespace}` : "Local shell";
}

/**
 * The namespace a new local shell uses: the first selected one, otherwise
 * (all namespaces) the context's own, otherwise `default`, as kubectl would.
 */
export function localShellNamespace(selected: readonly string[], contextNamespace: string | null | undefined): string {
  return selected[0] || contextNamespace || "default";
}

export function makeTab(spec: DockTabSpec): DockTab {
  return { ...spec, id: tabId(spec), title: tabTitle(spec) };
}

/** The tabs after opening `spec` (appended unless already open), and its id. */
export function openTab(tabs: DockTab[], spec: DockTabSpec): { tabs: DockTab[]; active: string } {
  const id = tabId(spec);
  if (tabs.some((t) => t.id === id)) return { tabs, active: id };
  return { tabs: [...tabs, makeTab(spec)], active: id };
}

/**
 * The tabs after closing `id`, and the tab to show next: the same one if
 * another tab was closed, otherwise its right neighbour (or the left one).
 */
export function closeTab(tabs: DockTab[], active: string | null, id: string): { tabs: DockTab[]; active: string | null } {
  const i = tabs.findIndex((t) => t.id === id);
  if (i < 0) return { tabs, active };
  const next = tabs.filter((t) => t.id !== id);
  if (active !== id) return { tabs: next, active };
  return { tabs: next, active: next[Math.min(i, next.length - 1)]?.id ?? null };
}

/** The dock height for a drag to `clientY`, kept between the minimum and `max`. */
export function clampHeight(height: number, max: number): number {
  return Math.round(Math.max(DOCK_MIN_HEIGHT, Math.min(height, Math.max(DOCK_MIN_HEIGHT, max))));
}

/** Deployments, StatefulSets, DaemonSets, Jobs and ReplicaSets have merged logs. */
export function isWorkloadResource(r: ResourceRef): boolean {
  return (r.group === "apps" || r.group === "batch") && WORKLOAD_KINDS.includes(r.kind);
}

/** The dock entries of a row's context menu: Logs and Shell for a pod, Logs for a workload. */
export function dockMenuFor(resource: ResourceRef, namespace: string, name: string): { label: string; spec: DockTabSpec }[] {
  if (resource.group === "" && resource.kind === "Pod") {
    const pod = { resource, namespace, name };
    return [
      { label: "Logs", spec: { type: "podLogs", pod } },
      { label: "Shell", spec: { type: "terminal", pod } },
    ];
  }
  if (isWorkloadResource(resource)) {
    return [{ label: "Logs", spec: { type: "workloadLogs", workload: { kind: resource.kind, namespace, name } } }];
  }
  return [];
}
