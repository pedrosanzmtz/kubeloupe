import type { ResourceRef } from "../bindings";

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

export function openDetail(target: Target) {
  detail.target = { resource: target.resource, namespace: target.namespace, name: target.name };
  detail.pod =
    target.resource.group === "" && target.resource.kind === "Pod"
      ? { namespace: target.namespace, name: target.name }
      : null;
  detail.nonce += 1;
}

export function closeDetail() {
  detail.target = null;
  detail.pod = null;
}

/** Row key of a target (`namespace/name`, or `name` if cluster scoped). */
export function targetKey(t: Target): string {
  return t.namespace ? `${t.namespace}/${t.name}` : t.name;
}
