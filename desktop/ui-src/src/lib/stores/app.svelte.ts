import { commands } from "../bindings";
import type { ContextInfo, NamespaceInfo, ResourceKind } from "../bindings";
import { closeDetail, openDetail } from "./detail.svelte";
import { closeAllDockTabs } from "./dock.svelte";
import { whenDiscarded } from "./unsaved.svelte";

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
  /** Listing namespaces is forbidden: the namespace control takes free text. */
  namespacesForbidden: false,
  /** Namespaces offered when they cannot be listed (context default + typed). */
  knownNamespaces: [] as string[],
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
  /** Ids of kinds a SelfSubjectAccessReview denied in `deniedScope`. */
  denied = $state.raw(new Set<string>());
  /** The namespace `denied` was checked in (`""` = all namespaces). */
  deniedScope = $state("");
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
  await checkAccess();
}

/**
 * RBAC: grey out what the user cannot list in the current namespace scope.
 * Best effort; re-run whenever the namespace changes.
 */
export async function checkAccess() {
  const found = kinds.all;
  const scope = app.namespace;
  if (found.length === 0) return;
  try {
    const access = await commands.cmdCheckAccess(
      found.map((k) => k.resource),
      scope || null,
    );
    // Ignore the answer if discovery or the namespace changed meanwhile.
    if (kinds.all === found && app.namespace === scope) {
      kinds.denied = new Set(access.filter((a) => !a.allowed).map((a) => a.id));
      kinds.deniedScope = scope;
    }
  } catch {
    // Without the check everything stays enabled.
  }
}

/** Why `kind` is greyed out, for its tooltip. */
export function deniedReason(kind: ResourceKind): string {
  if (!kind.resource.namespaced) return `You are not allowed to list ${kind.label}`;
  if (kinds.deniedScope) return `${kind.label}: not allowed in namespace ${kinds.deniedScope}`;
  return `${kind.label}: not allowed across all namespaces — pick a namespace`;
}

/**
 * Whether the sidebar should still open a denied kind: a namespaced kind
 * denied across all namespaces may be allowed in one, and its view has the
 * namespace control.
 */
export function canOpenDenied(kind: ResourceKind): boolean {
  return kind.resource.namespaced && !kinds.deniedScope;
}

/** Open an object in its own kind's view (owner links, related objects). */
export function openObject(kind: ResourceKind, namespace: string, name: string) {
  whenDiscarded(() => {
    setView(kind.id);
    openDetail({ resource: kind.resource, namespace, name });
  });
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

/** Typed namespaces, per context, for this session (persisting is #39). */
const typedNamespaces = new Map<string, string[]>();

function currentContextName(): string {
  return app.contexts.find((c) => c.current)?.name ?? "";
}

/** Offer `ns` in the namespace control when namespaces cannot be listed. */
export function rememberNamespace(ns: string) {
  if (!ns || app.knownNamespaces.includes(ns)) return;
  app.knownNamespaces = [...app.knownNamespaces, ns].sort();
  const ctx = currentContextName();
  typedNamespaces.set(ctx, [...(typedNamespaces.get(ctx) ?? []), ns]);
}

export async function loadNamespaces() {
  let listed: NamespaceInfo[] | null;
  try {
    listed = await commands.cmdListNamespaces();
  } catch {
    // Namespaces failing usually means the cluster is unreachable;
    // the main view load will surface the error.
    return;
  }
  app.namespaces = listed ?? [];
  app.namespacesForbidden = listed === null;
  const ctx = app.contexts.find((c) => c.current);
  app.knownNamespaces = [];
  if (listed === null) {
    for (const ns of [ctx?.namespace ?? "", ...(typedNamespaces.get(ctx?.name ?? "") ?? [])]) {
      if (ns && !app.knownNamespaces.includes(ns)) app.knownNamespaces.push(ns);
    }
    app.knownNamespaces.sort();
  }
}

/**
 * Pick the starting namespace: all namespaces if the user may list pods
 * cluster-wide, otherwise the context's namespace or the first one they can
 * access.
 */
export async function initScope() {
  let ns: string | null = null;
  try {
    ns = await commands.cmdDefaultNamespace();
  } catch {
    // Unreachable cluster: stay on all namespaces; the views report it.
  }
  app.namespace = ns ?? "";
  if (ns && app.namespacesForbidden && !app.knownNamespaces.includes(ns)) {
    app.knownNamespaces = [...app.knownNamespaces, ns].sort();
  }
}

/** Asks first if the detail panel has unsaved edits. */
export function setView(view: View) {
  whenDiscarded(() => {
    closeDetail();
    app.view = view;
    app.count = "";
    app.updated = "";
    showError("");
  });
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
  // The backend has stopped every stream and session of the old context.
  closeAllDockTabs();
  closeDetail();
  app.namespace = "";
  app.view = "overview";
  await loadContexts();
  await loadNamespaces();
  await initScope();
  refresh();
}

export function switchNamespace(namespace: string) {
  closeDetail();
  namespace = namespace.trim();
  if (app.namespacesForbidden) rememberNamespace(namespace);
  app.namespace = namespace;
}
