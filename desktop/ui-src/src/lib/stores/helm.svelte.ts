// State of the Helm releases view: the list (labels only), the charts
// (decoded lazily), the selected release and the rollback/uninstall dialog.

import { commands } from "../bindings";
import type { HelmChartInfo, HelmReleaseRow } from "../bindings";
import { chartKeys, releaseKey } from "../helm";
import { setNotice } from "./actions.svelte";

export interface HelmSelection {
  namespace: string;
  name: string;
}

export interface HelmActionRequest {
  action: "rollback" | "uninstall";
  namespace: string;
  name: string;
  /** Rollback: the revision to go back to. */
  revision: number;
  /** The release's current revision (for the dialog's text). */
  current: number;
  /** What the user typed to confirm (uninstall). */
  typed: string;
}

class HelmStore {
  rows = $state.raw<HelmReleaseRow[]>([]);
  /** Charts by release key, for the revision the row shows. */
  charts = $state.raw(new Map<string, HelmChartInfo>());
  warnings = $state.raw<string[]>([]);
  error = $state("");
  loaded = $state(false);
  selected = $state.raw<HelmSelection | null>(null);
  /** Bumped after a rollback, so the open detail reloads. */
  nonce = $state(0);
  /** `null` until checked; then the installed helm's version, or "". */
  helmVersion = $state<string | null>(null);
  /** Why helm cannot be used (not installed), once checked. */
  helmMissing = $state("");
  request = $state<HelmActionRequest | null>(null);
  busy = $state(false);
  /** Shown inside the dialog. */
  actionError = $state("");
}
export const helm = new HelmStore();

/** Ignores answers that a newer load overtook. */
let generation = 0;

/** List the releases in `namespaces` (empty: all), then fill in their charts. */
export async function loadReleases(namespaces: readonly string[]) {
  const gen = ++generation;
  let list;
  try {
    list = await commands.cmdHelmList([...namespaces]);
  } catch (e) {
    if (gen !== generation) return;
    helm.error = String(e);
    helm.rows = [];
    helm.warnings = [];
    helm.loaded = true;
    return;
  }
  if (gen !== generation) return;
  helm.error = "";
  helm.rows = list.releases;
  helm.warnings = list.warnings;
  helm.loaded = true;
  const sel = helm.selected;
  if (sel && !list.releases.some((r) => releaseKey(r) === releaseKey(sel))) helm.selected = null;
  await loadCharts(gen);
}

/** Decode the current revision of the rows whose chart is not known yet. */
async function loadCharts(gen: number) {
  const missing = helm.rows.filter((r) => helm.charts.get(releaseKey(r))?.revision !== r.revision);
  if (missing.length === 0) return;
  let charts: HelmChartInfo[];
  try {
    charts = await commands.cmdHelmCharts(chartKeys(missing));
  } catch {
    // The columns stay empty; the detail shows the chart anyway.
    return;
  }
  if (gen !== generation) return;
  const next = new Map(helm.charts);
  for (const c of charts) next.set(releaseKey(c), c);
  helm.charts = next;
}

export function selectRelease(sel: HelmSelection | null) {
  helm.selected = sel;
}

/** Forget the view's state when it closes. */
export function resetHelmView() {
  generation++;
  helm.selected = null;
  helm.request = null;
}

/** Check once per session whether the helm CLI is there. */
export async function checkHelm() {
  if (helm.helmVersion !== null) return;
  try {
    helm.helmVersion = await commands.cmdHelmVersion();
    helm.helmMissing = "";
  } catch (e) {
    helm.helmVersion = "";
    helm.helmMissing = String(e);
  }
}

export function requestHelmAction(req: Omit<HelmActionRequest, "typed">) {
  helm.request = { ...req, typed: "" };
  helm.busy = false;
  helm.actionError = "";
}

export function closeHelmDialog() {
  if (helm.busy) return;
  helm.request = null;
  helm.actionError = "";
}

export function canConfirmHelm(req: HelmActionRequest): boolean {
  if (helm.busy || helm.helmMissing) return false;
  if (req.action === "uninstall") return req.typed === req.name;
  return req.revision > 0 && req.revision !== req.current;
}

/** Run the confirmed action, then reload the list (and the open detail). */
export async function confirmHelmAction(namespaces: readonly string[]) {
  const req = helm.request;
  if (!req || !canConfirmHelm(req)) return;
  helm.busy = true;
  helm.actionError = "";
  try {
    const msg =
      req.action === "rollback"
        ? await commands.cmdHelmRollback(req.namespace, req.name, req.revision)
        : await commands.cmdHelmUninstall(req.namespace, req.name);
    helm.busy = false;
    helm.request = null;
    if (req.action === "uninstall" && helm.selected && releaseKey(helm.selected) === releaseKey(req)) {
      helm.selected = null;
    }
    setNotice(true, msg);
    helm.nonce += 1;
    await loadReleases(namespaces);
  } catch (e) {
    helm.busy = false;
    helm.actionError = String(e);
  }
}
