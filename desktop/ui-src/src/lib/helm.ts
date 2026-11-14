// Pure helpers for the Helm releases view (#46).

import type { HelmChartInfo, HelmReleaseRow, HelmRevisionKey } from "./bindings";

/** `app.view` of the Helm releases table. */
export const HELM_VIEW = "helm";

/** `namespace/name` of a release. */
export function releaseKey(r: { namespace: string; name: string }): string {
  return `${r.namespace}/${r.name}`;
}

/** `chart-version`, as `helm list` shows it; empty when unknown. */
export function chartLabel(chart: string, version: string): string {
  if (!chart) return "";
  return version ? `${chart}-${version}` : chart;
}

/** The revisions to fetch charts for: the current one of each release. */
export function chartKeys(rows: readonly HelmReleaseRow[]): HelmRevisionKey[] {
  return rows.map((r) => ({ namespace: r.namespace, name: r.name, revision: r.revision, storage: r.storage }));
}

/** Charts by release key, only for the revision each row shows. */
export function chartsByRelease(charts: readonly HelmChartInfo[]): Map<string, HelmChartInfo> {
  return new Map(charts.map((c) => [releaseKey(c), c]));
}

/** Rows matching the search text (name, namespace, status or chart), sorted by namespace then name. */
export function visibleReleases(
  rows: readonly HelmReleaseRow[],
  charts: ReadonlyMap<string, HelmChartInfo>,
  search: string,
): HelmReleaseRow[] {
  const q = search.trim().toLowerCase();
  const out = rows.filter((r) => {
    if (!q) return true;
    const c = charts.get(releaseKey(r));
    const hay = [r.name, r.namespace, r.status, c?.chart ?? "", c?.app_version ?? ""].join(" ").toLowerCase();
    return hay.includes(q);
  });
  return out.sort((a, b) => releaseKey(a).localeCompare(releaseKey(b)));
}

/** Colour class of a release status, like pod phases. */
export function statusClass(status: string): string {
  switch (status) {
    case "deployed":
      return "phase-Running";
    case "failed":
      return "phase-Failed";
    case "superseded":
    case "uninstalled":
      return "";
    default:
      // pending-install, pending-upgrade, pending-rollback, uninstalling
      return status ? "phase-Pending" : "";
  }
}

/**
 * The two revisions a diff starts with: the previous one against the one
 * shown. `history` is newest first. Null when there is only one revision.
 */
export function defaultDiffPair(history: readonly { revision: number }[], shown: number): [number, number] | null {
  const older = history.map((h) => h.revision).filter((r) => r < shown);
  if (older.length === 0) {
    const newer = history.map((h) => h.revision).filter((r) => r > shown);
    return newer.length ? [shown, Math.min(...newer)] : null;
  }
  return [Math.max(...older), shown];
}

/** Revisions a rollback can go to: every stored one except the current. */
export function rollbackTargets(history: readonly { revision: number }[], current: number): number[] {
  return history.map((h) => h.revision).filter((r) => r !== current);
}

/** Age text ("5m", "3d") for unix seconds; empty when unknown. */
export function ageOf(secs: number, nowMs: number): string {
  if (!secs) return "";
  const s = Math.max(0, Math.floor(nowMs / 1000 - secs));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}
