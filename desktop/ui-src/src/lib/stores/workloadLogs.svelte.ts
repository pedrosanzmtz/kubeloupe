import { commands } from "../bindings";

/** A workload whose pods' logs are merged into one stream. */
export interface WorkloadRef {
  /** Deployment, StatefulSet, DaemonSet, Job or ReplicaSet (case-insensitive; `deploy`, `sts`, `ds` also work). */
  kind: string;
  namespace: string;
  name: string;
}

/** The workload shown in the workload logs drawer, if any. */
export const workloadLogs = $state({
  target: null as WorkloadRef | null,
  /** Bumped on every open so re-opening the same workload restarts the stream. */
  nonce: 0,
});

/**
 * Open the merged logs of a Deployment / StatefulSet / DaemonSet / Job /
 * ReplicaSet in the logs drawer (mounted by `WorkloadLogsHost`). Replaces
 * any other running log stream.
 */
export function openWorkloadLogs(target: WorkloadRef) {
  workloadLogs.target = { kind: target.kind, namespace: target.namespace, name: target.name };
  workloadLogs.nonce += 1;
}

/** Close the drawer and stop the stream (and every per-pod task behind it). */
export function closeWorkloadLogs() {
  if (!workloadLogs.target) return;
  workloadLogs.target = null;
  commands.cmdStopWorkloadLogs().catch(() => {});
}
