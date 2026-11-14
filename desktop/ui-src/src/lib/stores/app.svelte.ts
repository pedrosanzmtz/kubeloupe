import { commands } from "../bindings";
import type { ContextInfo, NamespaceInfo, ResourceKind } from "../bindings";
import { closeDetail, openDetail } from "./detail.svelte";

/** `"overview"`, or the id of a discovered kind (`apps/v1/deployments`). */
export type View = string;

export interface Status {
  /** `null` while connecting, otherwise whether the cluster is reachable. */
  ok: boolean | null;
  text: string;
}

/** Shell state shared by the header, sidebar, banner and views. */
export const app = $state({
  view: "overview" as View,
  namespace: "",
  search: "",
  contexts: [] as ContextInfo[],
  namespaces: [] as NamespaceInfo[],
  status: { ok: null, text: "Connecting…" } as Status,
  error: "",
  /** Footer: item count and last update, set by the active view. */
  count: "",
  updated: "",
  /** Bumped by the Refresh button; views re-request their data on change. */
  refreshNonce: 0,
});

/** Discovery result (sidebar) and the kinds the user may not list. */
class Kinds {
  /** Replaced, never mutated: no need for deep reactivity on column defs. */
  all = $state.raw<ResourceKind[]>([]);
  /** Ids of kinds a SelfSubjectAccessReview denied. */
  denied = $state.raw(new Set<string>());
  /** Discovery finished (successfully or not). */
  loaded = $state(false);
}
export const kinds = new Kinds();

/** The kind of the current view, if it is one. */
export function currentKind(): ResourceKind | undefined {
  return kinds.all.find((k) => k.id === app.view);
}

export function kindOf(apiVersion: string, kind: string): ResourceKind | undefined {
  const [group, version] = apiVersion.includes("/") ? apiVersion.split("/") : ["", apiVersion];
  return (
    kinds.all.find(
      (k) => k.resource.group === group && k.resource.kind === kind && k.resource.version === version,
    ) ?? kinds.all.find((k) => k.resource.group === group && k.resource.kind === kind)
  );
}

/** Ask the cluster what it serves; the sidebar is built from the answer. */
export async function loadKinds() {
  let found: ResourceKind[];
  try {
    found = await commands.cmdDiscover();
  } catch (e) {
    kinds.all = [];
    kinds.loaded = true;
    showError("Could not discover the cluster's resource types: " + e);
    return;
  }
  kinds.all = found;
  kinds.denied = new Set();
  kinds.loaded = true;
  // RBAC: grey out what the user cannot list. Best effort, in the background.
  try {
    const access = await commands.cmdCheckAccess(found.map((k) => k.resource));
    // Ignore the answer if a newer discovery replaced this one meanwhile.
    if (kinds.all === found) {
      kinds.denied = new Set(access.filter((a) => !a.allowed).map((a) => a.id));
    }
  } catch {
    // Without the check everything stays enabled.
  }
}

/** Open an object in its own kind's view (owner links, related objects). */
export function openObject(kind: ResourceKind, namespace: string, name: string) {
  setView(kind.id);
  openDetail({ resource: kind.resource, namespace, name });
}

export function setStatus(ok: boolean | null, text: string) {
  app.status = { ok, text };
}

export function showError(message: string) {
  app.error = message;
}

export function stamp() {
  app.updated = "Updated " + new Date().toLocaleTimeString();
}

export async function loadContexts() {
  try {
    app.contexts = await commands.cmdListContexts();
  } catch {
    // No kubeconfig contexts (e.g. in-cluster): the selector stays hidden.
    app.contexts = [];
  }
}

export async function loadNamespaces() {
  try {
    app.namespaces = await commands.cmdListNamespaces();
  } catch {
    // Namespaces failing usually means the cluster is unreachable;
    // the main view load will surface the error.
  }
}

export function setView(view: View) {
  closeDetail();
  app.view = view;
  app.count = "";
  app.updated = "";
  showError("");
}

export function refresh() {
  app.refreshNonce += 1;
}

export async function switchContext(name: string) {
  try {
    await commands.cmdSetContext(name || null);
  } catch (e) {
    showError("Could not switch context: " + e);
    return;
  }
  closeDetail();
  app.namespace = "";
  app.view = "overview";
  await loadNamespaces();
  refresh();
}

export function switchNamespace(namespace: string) {
  closeDetail();
  app.namespace = namespace;
}
