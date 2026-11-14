import { commands } from "../bindings";
import type { ContextInfo, NamespaceInfo, ResourceKind } from "../bindings";
import { joinNames, normalizeNamespaces, sameNamespaces } from "../namespaces";
import { clampHeight } from "../dock";
import { PORT_FORWARDS_VIEW } from "../forwards";
import { contextToRestore, namespacesToRestore } from "../settings";
import { closeDetail, detail, openDetail } from "./detail.svelte";
import { contextSettings, loadSettings, settings, updateContext, updateSettings } from "./settings.svelte";
import { closeAllDockTabs, dock } from "./dock.svelte";
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
  /**
   * The namespaces the views show, in the order picked (at most
   * `MAX_NAMESPACES`); empty means all namespaces. Replaced, never mutated.
   */
  selectedNamespaces: [] as string[],
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
  /** The remembered context and namespaces are restored (`startApp`). */
  ready: false,
});

/** The view to open once discovery shows whether its kind still exists. */
let pendingView: View | null = null;

/** Discovery result (sidebar) and the kinds the user may not list. */
class Kinds {
  /** Replaced, never mutated: no need for deep reactivity on column defs. */
  all = $state.raw<ResourceKind[]>([]);
  /** Ids of kinds a SelfSubjectAccessReview denied in `deniedScope`. */
  denied = $state.raw(new Set<string>());
  /** The namespaces `denied` was checked in (empty = all namespaces). */
  deniedScope = $state.raw<string[]>([]);
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
  restorePendingView();
}

/** Open the remembered view if its kind is still served and allowed. */
function restorePendingView() {
  const view = pendingView;
  pendingView = null;
  if (!view || view === "overview" || app.view !== "overview") return;
  if (view === PORT_FORWARDS_VIEW) {
    app.view = view;
    return;
  }
  const kind = kinds.all.find((k) => k.id === view);
  if (kind && (!kinds.denied.has(kind.id) || canOpenDenied(kind))) app.view = view;
}

/**
 * RBAC: grey out what the user cannot list in the current namespace scope:
 * with several namespaces, what is denied in all of them. Best effort;
 * re-run whenever the selection changes.
 */
export async function checkAccess() {
  const found = kinds.all;
  const scope = [...app.selectedNamespaces];
  if (found.length === 0) return;
  try {
    const resources = found.map((k) => k.resource);
    const answers = await Promise.all(
      (scope.length ? scope : [null]).map((ns) => commands.cmdCheckAccess(resources, ns)),
    );
    // Ignore the answer if discovery or the namespaces changed meanwhile.
    if (kinds.all === found && sameNamespaces(app.selectedNamespaces, scope)) {
      const deniedIn = answers.map((access) => new Set(access.filter((a) => !a.allowed).map((a) => a.id)));
      kinds.denied = new Set(found.map((k) => k.id).filter((id) => deniedIn.every((d) => d.has(id))));
      kinds.deniedScope = scope;
    }
  } catch {
    // Without the check everything stays enabled.
  }
}

/** Why `kind` is greyed out, for its tooltip. */
export function deniedReason(kind: ResourceKind): string {
  if (!kind.resource.namespaced) return `You are not allowed to list ${kind.label}`;
  const scope = kinds.deniedScope;
  if (scope.length === 1) return `${kind.label}: not allowed in namespace ${scope[0]}`;
  if (scope.length > 1) return `${kind.label}: not allowed in namespaces ${joinNames(scope)}`;
  return `${kind.label}: not allowed across all namespaces — pick a namespace`;
}

/**
 * Whether the sidebar should still open a denied kind: a namespaced kind
 * denied across all namespaces may be allowed in one, and its view has the
 * namespace control.
 */
export function canOpenDenied(kind: ResourceKind): boolean {
  return kind.resource.namespaced && kinds.deniedScope.length === 0;
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

function currentContextName(): string {
  return app.contexts.find((c) => c.current)?.name ?? "";
}

/** Offer `ns` in the namespace control when namespaces cannot be listed. */
export function rememberNamespace(ns: string) {
  if (!ns || app.knownNamespaces.includes(ns)) return;
  app.knownNamespaces = [...app.knownNamespaces, ns].sort();
  const ctx = currentContextName();
  const typed = contextSettings(ctx).typed_namespaces;
  if (!typed.includes(ns)) updateContext(ctx, { typed_namespaces: [...typed, ns] });
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
    for (const ns of [ctx?.namespace ?? "", ...contextSettings(ctx?.name ?? "").typed_namespaces]) {
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
  app.selectedNamespaces = ns ? [ns] : [];
  if (ns && app.namespacesForbidden && !app.knownNamespaces.includes(ns)) {
    app.knownNamespaces = [...app.knownNamespaces, ns].sort();
  }
}

/**
 * Restore what was remembered for the current context: its namespaces (or
 * the default scope) now, its view once discovery has run.
 */
async function restoreScope() {
  const saved = contextSettings(currentContextName());
  const listed = app.namespaces.map((n) => n.name);
  const namespaces = namespacesToRestore(saved.namespaces, listed, app.namespacesForbidden);
  if (namespaces) app.selectedNamespaces = namespaces;
  else await initScope();
  pendingView = saved.view || null;
}

/**
 * Start up: load the settings, switch to the remembered context if it still
 * exists, and restore its namespaces and view.
 */
export async function startApp() {
  await loadSettings();
  // Leave room for the header and the tables, as a drag does.
  const docked = settings.current.dock_height;
  if (docked > 0) dock.height = clampHeight(docked, window.innerHeight - 160);
  await loadContexts();
  const remembered = contextToRestore(settings.current.last_context, app.contexts);
  if (remembered) {
    try {
      await commands.cmdSetContext(remembered);
      await loadContexts();
    } catch {
      // Stay on the kubeconfig's current-context.
    }
  }
  await loadNamespaces();
  await restoreScope();
  app.ready = true;
}

/** Asks first if the detail panel has unsaved edits. */
export function setView(view: View) {
  whenDiscarded(() => {
    closeDetail();
    app.view = view;
    pendingView = null;
    updateContext(currentContextName(), { view });
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
  app.selectedNamespaces = [];
  app.view = "overview";
  updateSettings({ last_context: name });
  await loadContexts();
  await loadNamespaces();
  await restoreScope();
  refresh();
}

/**
 * Show `namespaces` (empty: all namespaces). Typed namespaces are offered
 * again when namespaces cannot be listed; the open detail closes when its
 * object's namespace is no longer shown.
 */
export function setNamespaces(namespaces: readonly string[]) {
  const next = normalizeNamespaces(namespaces);
  if (sameNamespaces(next, app.selectedNamespaces)) return;
  if (app.namespacesForbidden) next.forEach(rememberNamespace);
  const open = detail.target?.namespace;
  if (open && next.length > 0 && !next.includes(open)) closeDetail();
  app.selectedNamespaces = next;
  updateContext(currentContextName(), { namespaces: next });
}

/** Show just `namespace` (`""`: all namespaces). */
export function switchNamespace(namespace: string) {
  setNamespaces(namespace.trim() ? [namespace] : []);
}

/**
 * The one namespace for what takes a single one (the overview's fallback
 * scope, the create dialog's default): the first selected, `""` for all.
 */
export function primaryNamespace(): string {
  return app.selectedNamespaces[0] ?? "";
}
