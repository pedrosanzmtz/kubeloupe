// What `cmd_get_related` returns in the fake cluster, by `kindId|rowKey`.
// Objects without an entry get `NO_RELATED` (like a kind the backend has no
// relations for).

import type { Related, RelatedGroup, RelatedItem } from "../../lib/bindings";

export const NO_RELATED: Related = { summary: [], facts: [], groups: [] };

/** A group of objects of `kind`; each row is `[name, ...cells]`. */
export function objects(
  title: string,
  apiVersion: string,
  kind: string,
  columns: string[],
  rows: { namespace: string; cells: string[] }[],
): RelatedGroup {
  const items: RelatedItem[] = rows.map((r) => ({ namespace: r.namespace, name: r.cells[0], cells: r.cells }));
  return { title, api_version: apiVersion, kind, columns, name_column: 0, items, error: "" };
}

const POD_COLUMNS = ["Name", "Ready", "Status", "Restarts", "Age"];

export function relatedFixtures(): Record<string, Related> {
  return {
    "apps/v1/deployments|demo/web": {
      summary: ["2/2 ready", "RollingUpdate 25%/25%", "revision 4"],
      facts: [
        { label: "Rollout", value: "complete" },
        { label: "Generation", value: "observed 4 of 4" },
      ],
      groups: [
        objects(
          "ReplicaSets",
          "apps/v1",
          "ReplicaSet",
          ["Name", "Revision", "Ready", "Age"],
          [{ namespace: "demo", cells: ["web-5d8f", "4", "2/2", "1h"] }],
        ),
        objects("Pods", "v1", "Pod", POD_COLUMNS, [
          { namespace: "demo", cells: ["web-1", "1/1", "Running", "0", "1h"] },
          { namespace: "demo", cells: ["web-2", "0/1", "CrashLoopBackOff", "3", "1h"] },
        ]),
      ],
    },
    "v1/nodes|node-1": {
      summary: ["Ready", "v1.37.0"],
      facts: [{ label: "InternalIP", value: "172.18.0.2" }],
      groups: [
        objects(
          "Pods",
          "v1",
          "Pod",
          ["Name", "Namespace", "Ready", "Status", "Restarts"],
          [{ namespace: "demo", cells: ["web-1", "demo", "1/1", "Running", "0"] }],
        ),
        {
          title: "Allocated resources",
          api_version: "",
          kind: "",
          columns: ["Resource", "Requests", "Limits", "Allocatable"],
          name_column: 0,
          items: [{ namespace: "", name: "", cells: ["cpu", "750m (37%)", "1 (50%)", "2"] }],
          error: "",
        },
        objects("Taints", "", "", ["Key", "Value", "Effect"], []),
      ],
    },
    "v1/configmaps|demo/settings": {
      summary: ["2 keys"],
      facts: [],
      groups: [
        {
          ...objects("Used by pods", "v1", "Pod", ["Name", "Ready", "Status", "Used as"], []),
          error: "pods is forbidden: User cannot list pods in the namespace demo",
        },
      ],
    },
    "v1/pods|demo/web-1": {
      summary: [],
      facts: [],
      groups: [
        objects(
          "ConfigMaps",
          "v1",
          "ConfigMap",
          ["Name", "Used as"],
          [{ namespace: "demo", cells: ["settings", "volume config, envFrom (app)"] }],
        ),
        objects(
          "Secrets",
          "v1",
          "Secret",
          ["Name", "Used as"],
          [{ namespace: "demo", cells: ["db-creds", "env PASSWORD (app)"] }],
        ),
      ],
    },
  };
}
