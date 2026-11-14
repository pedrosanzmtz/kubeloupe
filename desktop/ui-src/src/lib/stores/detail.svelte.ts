import type { ResourceRef } from "../bindings";
import { whenDiscarded } from "./unsaved.svelte";

/** The object shown in the detail panel. */
export interface Target {
  resource: ResourceRef;
  /** Empty for cluster-scoped kinds. */
  namespace: string;
  name: string;
}

export interface PodRef {
  namespace: string;
  name: string;
}

export const detail = $state({
  target: null as Target | null,
  /** The same object when it is a pod (the log viewer watches this), else null. */
  pod: null as PodRef | null,
  /** Bumped on every open so re-clicking the same object reloads the panel. */
  nonce: 0,
});

/** Re-opening the object already shown keeps its unsaved edits (they are
 *  restored); any other object asks before dropping them. */
export function openDetail(target: Target) {
  const same = detail.target !== null && sameTarget(detail.target, target);
  (same ? (fn: () => void) => fn() : whenDiscarded)(() => {
    detail.target = { resource: target.resource, namespace: target.namespace, name: target.name };
    detail.pod =
      target.resource.group === "" && target.resource.kind === "Pod"
        ? { namespace: target.namespace, name: target.name }
        : null;
    detail.nonce += 1;
  });
}

/** Asks first if the panel has unsaved edits. */
export function closeDetail() {
  whenDiscarded(() => {
    detail.target = null;
    detail.pod = null;
  });
}

function sameTarget(a: Target, b: Target): boolean {
  return (
    a.resource.group === b.resource.group &&
    a.resource.kind === b.resource.kind &&
    a.namespace === b.namespace &&
    a.name === b.name
  );
}

/** Row key of a target (`namespace/name`, or `name` if cluster scoped). */
export function targetKey(t: Target): string {
  return t.namespace ? `${t.namespace}/${t.name}` : t.name;
}
