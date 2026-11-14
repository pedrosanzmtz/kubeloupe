// The default in-memory cluster of the fake backend. Plain data: tests change
// it through `window.__fake.cluster` (or `window.__fakeInit`, before the app
// mounts) instead of adding special cases to the fake.

import type {
  ColumnKind,
  ContextInfo_Serialize,
  FieldTakeover,
  MetricsSnapshot,
  NamespaceInfo,
  Overview_Serialize,
  PodDetail,
  ResourceKind,
  ResourceRow,
} from "../../lib/bindings";
import { DISCOVERED } from "./discover";

export interface FakeCluster {
  contexts: ContextInfo_Serialize[];
  /** `null`: listing namespaces is forbidden (a namespace-scoped user). */
  namespaces: NamespaceInfo[] | null;
  /** What `cmd_default_namespace` returns (`null` = all namespaces). */
  defaultNamespace: string | null;
  /** What `cmd_discover` returns. */
  kinds: ResourceKind[];
  /** Kind ids `cmd_check_access` denies, in every scope. */
  denied: string[];
  /**
   * Kind ids denied across all namespaces only: `cmd_check_access` denies
   * them without a namespace, and their watches end with a 403 there.
   */
  deniedClusterWide: string[];
  /** Rows by kind id; watches serve these. */
  rows: Record<string, ResourceRow[]>;
  /** Pod details by `namespace/name`. */
  pods: Record<string, PodDetail>;
  /** YAML by `kindId|rowKey`; missing objects get a generated manifest. */
  yaml: Record<string, string>;
  /** Secret YAML with values, by `kindId|rowKey` (`reveal: true`). */
  revealedYaml: Record<string, string>;
  /** Returned (once) by the next `cmd_apply_resource_yaml`. */
  nextTakeovers: FieldTakeover[];
  /** `metadata.resourceVersion` by `kindId|rowKey` (default 1); a save bumps it. Bump it to
   *  simulate a write by someone else. */
  resourceVersions: Record<string, number>;
  /** `cmd_bulk_delete` fails these row keys with the given reason. */
  bulkErrors: Record<string, string>;
  /** `cmd_apply_manifests` rejects objects with these names (dry run included), with the given reason. */
  applyErrors: Record<string, string>;
  overview: Overview_Serialize;
  metrics: MetricsSnapshot;
}

/** Unix seconds, a fixed distance in the past so ages are stable. */
const ago = (secs: number) => Math.floor(Date.now() / 1000) - secs;

/** A row for `kindId` with cells given by column key (missing cells are empty). */
export function makeRow(
  kinds: ResourceKind[],
  kindId: string,
  namespace: string,
  name: string,
  cells: Record<string, string> = {},
  created = ago(3600),
): ResourceRow {
  const kind = kinds.find((k) => k.id === kindId);
  if (!kind) throw new Error(`fake cluster: no kind ${kindId}`);
  const values: Record<string, string> = { name, namespace, ...cells };
  return {
    key: namespace ? `${namespace}/${name}` : name,
    name,
    namespace,
    created,
    // Like the backend: columns the UI fills itself get an empty cell.
    cells: kind.columns.map((c) => (UI_FILLED.has(c.kind) ? "" : (values[c.key] ?? ""))),
  };
}

const UI_FILLED = new Set<ColumnKind>(["age", "podCpu", "podMemory", "nodeCpu", "nodeMemory"]);

/** A CRD, so the sidebar's "Custom Resources" section has something in it. */
const WIDGETS: ResourceKind = {
  resource: { group: "example.com", version: "v1", kind: "Widget", plural: "widgets", namespaced: true },
  id: "example.com/v1/widgets",
  label: "Widgets",
  category: "custom",
  crd: true,
  verbs: ["get", "list", "watch", "delete"],
  short_names: [],
  columns: [
    { key: "name", label: "Name", kind: "text" },
    { key: "namespace", label: "Namespace", kind: "text" },
    { key: "age", label: "Age", kind: "age" },
  ],
  actions: [{ id: "delete", label: "Delete", destructive: true, type_to_confirm: false, input: "none" }],
};

export function defaultCluster(): FakeCluster {
  const kinds = [...DISCOVERED, WIDGETS];
  const row = (kindId: string, ns: string, name: string, cells: Record<string, string> = {}, created?: number) =>
    makeRow(kinds, kindId, ns, name, cells, created);
  const pod = (ns: string, name: string, status: string, restarts = "0", node = "node-1") =>
    row("v1/pods", ns, name, { ready: status === "Running" ? "1/1" : "0/1", status, restarts, node });

  return {
    contexts: [
      { name: "kind-dev", current: true, namespace: null },
      { name: "prod", current: false, namespace: null },
    ],
    defaultNamespace: null,
    namespaces: ["default", "demo", "kube-system"].map((name) => ({
      name,
      status: "Active",
      created: ago(86400 * 10),
      age: "10d",
    })),
    kinds,
    denied: ["rbac.authorization.k8s.io/v1/clusterroles", "v1/componentstatuses"],
    deniedClusterWide: [],
    rows: {
      "v1/pods": [
        pod("demo", "web-1", "Running"),
        pod("demo", "web-2", "Running", "3"),
        pod("demo", "db-0", "Pending"),
        pod("demo", "crashed", "CrashLoopBackOff", "12", "node-2"),
        pod("kube-system", "coredns-1", "Running"),
      ],
      "apps/v1/deployments": [
        row("apps/v1/deployments", "demo", "web", { ready: "2/2", uptodate: "2", available: "2" }),
        row("apps/v1/deployments", "demo", "api", { ready: "1/3", uptodate: "3", available: "1" }),
      ],
      "v1/nodes": [
        row("v1/nodes", "", "node-1", { ready: "True", version: "v1.37.0" }),
        row("v1/nodes", "", "node-2", { ready: "True", version: "v1.37.0" }),
      ],
      "v1/secrets": [row("v1/secrets", "demo", "db-creds", { type: "Opaque", data: "1" })],
      "v1/configmaps": [row("v1/configmaps", "demo", "settings", { data: "2" })],
      "example.com/v1/widgets": [row("example.com/v1/widgets", "demo", "gizmo")],
    },
    pods: {
      "demo/web-1": {
        name: "web-1",
        namespace: "demo",
        phase: "Running",
        node: "node-1",
        ip: "10.244.0.12",
        age: "1h",
        labels: { app: "web" },
        containers: [
          { name: "app", image: "nginx:1.27", ready: true, restarts: 0, state: "running" },
          { name: "sidecar", image: "busybox:1.36", ready: true, restarts: 0, state: "running" },
        ],
        log_containers: [
          { name: "init-db", init: true },
          { name: "app", init: false },
          { name: "sidecar", init: false },
        ],
      },
    },
    yaml: {},
    revealedYaml: {
      "v1/secrets|demo/db-creds": [
        "apiVersion: v1",
        "kind: Secret",
        "metadata:",
        "  name: db-creds",
        "  namespace: demo",
        "type: Opaque",
        "stringData:",
        "  password: hunter2",
        "",
      ].join("\n"),
    },
    nextTakeovers: [],
    resourceVersions: {},
    bulkErrors: {},
    applyErrors: {},
    overview: {
      namespaces: 3,
      nodes: { total: 2, ready: 2 },
      pods: {
        namespace: null,
        total: 5,
        running: 3,
        pending: 1,
        failed: 1,
        by_namespace: [
          ["demo", 4],
          ["kube-system", 1],
        ],
      },
      events: { namespace: null, items: [] },
      notes: [],
    },
    metrics: {
      available: true,
      pods: [
        { namespace: "demo", name: "web-1", cpu_millicores: 250, memory_bytes: 64 * 1024 * 1024 },
        { namespace: "demo", name: "web-2", cpu_millicores: 1500, memory_bytes: 1536 * 1024 * 1024 },
      ],
      nodes: [
        {
          name: "node-1",
          cpu_millicores: 900,
          cpu_capacity_millicores: 4000,
          memory_bytes: 2 * 1024 ** 3,
          memory_capacity_bytes: 8 * 1024 ** 3,
        },
      ],
    },
  };
}
