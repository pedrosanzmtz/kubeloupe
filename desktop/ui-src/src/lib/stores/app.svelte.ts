import { commands } from "../bindings";
import type { ContextInfo, NamespaceInfo } from "../bindings";
import { closeDetail } from "./detail.svelte";

export type View = "overview" | "pods" | "namespaces" | "nodes";

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
  await loadNamespaces();
  refresh();
}

export function switchNamespace(namespace: string) {
  closeDetail();
  app.namespace = namespace;
}
