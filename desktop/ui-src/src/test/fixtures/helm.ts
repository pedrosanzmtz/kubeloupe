// Helm releases for the fake backend: each revision fully decoded, so the
// list, the charts and the detail are all derived from one place.

import type { HelmRelease, HelmReleaseList, HelmReleaseRow } from "../../lib/bindings";

export const HIDDEN = "<hidden: reveal to view>";

export interface FakeHelm {
  /** Every stored revision, any order. */
  revisions: HelmRelease[];
  /** Manifests with Secret values, by `namespace/name@revision` (`reveal: true`). */
  revealed: Record<string, string>;
  /** Namespaces whose Secrets may not be listed (a warning each). */
  forbidden: string[];
  /** What `cmd_helm_version` returns; empty: helm is not installed. */
  version: string;
}

const now = () => Math.floor(Date.now() / 1000);

function manifest(name: string, greeting: string, password: string): string {
  return `---
# Source: web/templates/secret.yaml
apiVersion: v1
kind: Secret
metadata:
  name: ${name}-secret
stringData:
  password: '${password}'
---
# Source: web/templates/configmap.yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: ${name}-config
data:
  greeting: "${greeting}"
---
# Source: web/templates/deployment.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: ${name}
`;
}

export function revision(
  namespace: string,
  name: string,
  rev: number,
  status: string,
  greeting: string,
  chart = "web",
): HelmRelease {
  return {
    name,
    namespace,
    revision: rev,
    status,
    storage: "secret",
    chart,
    chart_version: "1.2.0",
    app_version: "2.4",
    description: rev === 1 ? "Install complete" : "Upgrade complete",
    notes: `${name} says ${greeting}.\n`,
    first_deployed: "2026-11-01T10:00:00Z",
    last_deployed: `2026-11-0${Math.min(rev, 9)}T10:00:00Z`,
    values: greeting === "hello" ? "" : `greeting: ${greeting}\n`,
    computed_values: `greeting: ${greeting}\nreplicas: 1\n`,
    manifest: manifest(name, greeting, HIDDEN),
    secrets_hidden: true,
    resources: [
      { api_version: "v1", kind: "Secret", namespace, name: `${name}-secret` },
      { api_version: "v1", kind: "ConfigMap", namespace, name: `${name}-config` },
      { api_version: "apps/v1", kind: "Deployment", namespace, name },
    ],
    history: [],
  };
}

export function defaultHelm(): FakeHelm {
  const revisions = [
    revision("demo", "web", 1, "superseded", "hello"),
    revision("demo", "web", 2, "deployed", "bye"),
    revision("kube-system", "metrics", 1, "failed", "hello", "metrics-server"),
  ];
  const revealed: Record<string, string> = {};
  for (const r of revisions) {
    revealed[`${r.namespace}/${r.name}@${r.revision}`] = manifest(r.name, r.notes.includes("bye") ? "bye" : "hello", "s3cret");
  }
  return { revisions, revealed, forbidden: [], version: "v3.16.4+g7877b45" };
}

const key = (r: { namespace: string; name: string }) => `${r.namespace}/${r.name}`;

/** The releases, as `cmd_helm_list` returns them. */
export function helmList(h: FakeHelm, namespaces: string[]): HelmReleaseList {
  const warnings = namespaces
    .filter((ns) => h.forbidden.includes(ns))
    .map((ns) => `Not allowed to list Secrets in namespace ${ns}, where Helm keeps its releases: its releases are not shown.`);
  const shown = h.revisions.filter(
    (r) => (namespaces.length === 0 || namespaces.includes(r.namespace)) && !h.forbidden.includes(r.namespace),
  );
  const byRelease = new Map<string, HelmRelease[]>();
  for (const r of shown) byRelease.set(key(r), [...(byRelease.get(key(r)) ?? []), r]);
  const releases: HelmReleaseRow[] = [...byRelease.values()].map((revs) => {
    revs.sort((a, b) => b.revision - a.revision);
    const top = revs[0];
    return {
      name: top.name,
      namespace: top.namespace,
      revision: top.revision,
      status: top.status,
      updated: now() - 3600 * (10 - Math.min(top.revision, 9)),
      storage: top.storage,
      history: revs.map((r) => ({ revision: r.revision, status: r.status, updated: now() - 3600 })),
    };
  });
  releases.sort((a, b) => key(a).localeCompare(key(b)));
  return { releases, warnings };
}

/** One revision with its history, as `cmd_helm_get` returns it. */
export function helmGet(h: FakeHelm, namespace: string, name: string, rev: number, reveal: boolean): HelmRelease {
  const row = helmList(h, [namespace]).releases.find((r) => r.name === name);
  if (!row) throw `No Helm release named ${name} in namespace ${namespace}`;
  const want = rev || row.revision;
  const r = h.revisions.find((x) => x.namespace === namespace && x.name === name && x.revision === want);
  if (!r) throw `Release ${name} has no revision ${want}`;
  const revealed = h.revealed[`${namespace}/${name}@${want}`];
  return {
    ...r,
    history: row.history,
    manifest: reveal && revealed ? revealed : r.manifest,
    secrets_hidden: !reveal,
  };
}

/** `helm rollback`: a new revision copying `to`, the old current one superseded. */
export function helmRollback(h: FakeHelm, namespace: string, name: string, to: number): string {
  const row = helmList(h, [namespace]).releases.find((r) => r.name === name);
  const src = h.revisions.find((x) => x.namespace === namespace && x.name === name && x.revision === to);
  if (!row || !src) throw `Rollback of ${name} failed: release: not found`;
  for (const r of h.revisions) if (key(r) === key(src) && r.status === "deployed") r.status = "superseded";
  const next = { ...src, revision: row.revision + 1, status: "deployed", description: `Rollback to ${to}` };
  h.revisions.push(next);
  const revealed = h.revealed[`${namespace}/${name}@${to}`];
  if (revealed) h.revealed[`${namespace}/${name}@${next.revision}`] = revealed;
  return `Rolled back ${namespace}/${name} to revision ${to}`;
}

export function helmUninstall(h: FakeHelm, namespace: string, name: string): string {
  const before = h.revisions.length;
  h.revisions = h.revisions.filter((r) => !(r.namespace === namespace && r.name === name));
  if (h.revisions.length === before) throw `Uninstall of ${name} failed: release: not found`;
  return `Uninstalled ${namespace}/${name}`;
}
