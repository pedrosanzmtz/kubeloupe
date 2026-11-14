import type { ActionDef, ActionId, DrainEvent, ResourceRef } from "./bindings";
import type { Row } from "./columns";

/** One object an action is applied to. */
export interface ActionTarget {
  /** Empty for cluster-scoped kinds. */
  namespace: string;
  name: string;
}

/** How many names a confirmation lists before saying "and N more". */
const LISTED = 8;

export function targetLabel(t: ActionTarget): string {
  return t.namespace ? `${t.namespace}/${t.name}` : t.name;
}

/** A readable list of the exact objects: names, then "and N more". */
export function listTargets(targets: ActionTarget[]): string[] {
  const names = targets.slice(0, LISTED).map(targetLabel);
  if (targets.length > LISTED) names.push(`…and ${targets.length - LISTED} more`);
  return names;
}

/**
 * An action needs a dialog if it is destructive or asks for input, and
 * whenever it hits several objects at once.
 */
export function needsDialog(action: ActionDef, targets: ActionTarget[]): boolean {
  return action.destructive || action.input !== "none" || targets.length > 1;
}

/** Scaling to 0 stops the workload, so it reads as destructive. */
export function isDestructive(action: ActionDef, replicas: number): boolean {
  return action.destructive || (action.id === "scale" && replicas === 0);
}

const VERB: Record<ActionId, string> = {
  delete: "Delete",
  forceDelete: "Force delete",
  scale: "Scale",
  restart: "Restart",
  cordon: "Cordon",
  uncordon: "Uncordon",
  drain: "Drain",
  trigger: "Trigger",
  suspend: "Suspend",
  resume: "Resume",
  portForward: "Port forward",
  debug: "Debug",
  nodeShell: "Shell",
};

export interface Copy {
  title: string;
  /** The sentence above the list of objects. */
  lead: string;
  /** Warning shown under the list, if any. */
  warning: string;
  /** Text of the confirm button. */
  button: string;
}

/** The wording of the confirmation: names the kind, the count and the effect. */
export function confirmCopy(action: ActionDef, kind: string, count: number, replicas: number): Copy {
  const noun = count === 1 ? kind : `${count} ${kind}s`;
  const verb = VERB[action.id];
  switch (action.id) {
    case "delete":
      return {
        title: `Delete ${noun}`,
        lead: count === 1 ? `This permanently deletes the ${kind}:` : `This permanently deletes ${count} ${kind}s:`,
        warning: "",
        button: count === 1 ? "Delete" : `Delete ${count}`,
      };
    case "forceDelete":
      return {
        title: `Force delete ${noun}`,
        lead: "This removes the pod from the API immediately, with a grace period of 0:",
        warning:
          "Its containers may keep running on the node, and a StatefulSet pod may be replaced while the old one is still alive. Use this only for pods stuck terminating.",
        button: "Force delete",
      };
    case "drain":
      return {
        title: `Drain ${noun}`,
        lead: "This cordons the node, then evicts its pods (DaemonSet and mirror pods are left alone):",
        warning:
          "Pods without a controller are not recreated elsewhere. Evictions blocked by a PodDisruptionBudget are retried until the timeout.",
        button: "Drain",
      };
    case "scale":
      return {
        title: `Scale ${noun}`,
        lead: "Set the number of replicas of:",
        warning: replicas === 0 ? "Scaling to 0 stops every pod of this workload until it is scaled up again." : "",
        button: replicas === 0 ? "Scale to 0" : `Scale to ${replicas}`,
      };
    default:
      return { title: `${verb} ${noun}`, lead: `${verb}:`, warning: "", button: verb };
  }
}

/** "3/5" in a workload's ready column: the desired count is the second number. */
export function replicasFromRow(row: Row | undefined, columnKeys: string[]): number {
  const i = columnKeys.indexOf("ready");
  const m = i >= 0 ? /^(\d+)\/(\d+)$/.exec(row?.cells[i] ?? "") : null;
  return m ? Number(m[2]) : 1;
}

/** One progress line of a drain, in plain words. */
export function drainLine(ev: DrainEvent): string {
  switch (ev.type) {
    case "cordoned":
      return `Node ${ev.node} cordoned`;
    case "planned": {
      const extra = ev.unmanaged > 0 ? ` (${ev.unmanaged} without a controller will not be recreated)` : "";
      return `${ev.to_evict} pods to evict, ${ev.skipped} skipped${extra}`;
    }
    case "evicted":
      return `Evicted ${ev.namespace}/${ev.name}`;
    case "waitingOnPdb":
      return `${ev.namespace}/${ev.name}: blocked by a PodDisruptionBudget, retrying`;
    case "failed":
      return `${ev.namespace}/${ev.name}: failed. ${ev.error}`;
  }
}

export function sameResource(a: ResourceRef, b: ResourceRef): boolean {
  return a.group === b.group && a.version === b.version && a.kind === b.kind && a.plural === b.plural;
}
