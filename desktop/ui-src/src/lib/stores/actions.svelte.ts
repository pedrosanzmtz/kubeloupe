import { commands, events } from "../bindings";
import type { ActionDef, BulkItemResult, DrainSummary, ResourceKind, ResourceRef } from "../bindings";
import { drainLine, needsDialog, sameResource, targetLabel } from "../actions";
import type { ActionTarget } from "../actions";
import { kinds } from "./app.svelte";
import { closeDetail, detail } from "./detail.svelte";

/** Drain gives up waiting on PodDisruptionBudgets after this long. */
const DRAIN_TIMEOUT_SECS = 300;
const NOTICE_MS = 6000;

export interface ActionRequest {
  kind: ResourceKind;
  targets: ActionTarget[];
  action: ActionDef;
  /** Current value of the replicas field (scale). */
  replicas: number;
  /** What the user typed to confirm (force delete). */
  typed: string;
}

export interface DrainView {
  /** `null` until the backend has answered. */
  id: string | null;
  node: string;
  lines: string[];
  finished: boolean;
  error: string;
  summary: DrainSummary | null;
}

/** State of the action dialog and of the result banner. */
export const actionUi = $state({
  request: null as ActionRequest | null,
  busy: false,
  /** Shown inside the dialog. */
  error: "",
  /** Per-object failures of a bulk delete. */
  failures: [] as BulkItemResult[],
  /** Objects of a bulk delete that went through. */
  deleted: 0,
  drain: null as DrainView | null,
  /** Outcome of an action that ran without a dialog. */
  notice: null as { ok: boolean; text: string } | null,
});

let noticeTimer: ReturnType<typeof setTimeout> | null = null;

export function setNotice(ok: boolean, text: string) {
  if (noticeTimer) clearTimeout(noticeTimer);
  actionUi.notice = { ok, text };
  // Failures stay until dismissed.
  noticeTimer = ok ? setTimeout(() => (actionUi.notice = null), NOTICE_MS) : null;
}

export function dismissNotice() {
  actionUi.notice = null;
}

/** The discovered kind of an object reference (for the detail panels). */
export function kindOfResource(r: ResourceRef): ResourceKind | undefined {
  return kinds.all.find((k) => sameResource(k.resource, r));
}

/**
 * Ask to run `action` on `targets`: opens the confirmation dialog for
 * destructive actions, actions with input and bulk actions, runs the rest
 * right away.
 */
export function requestAction(kind: ResourceKind, targets: ActionTarget[], action: ActionDef, replicas = 1) {
  if (targets.length === 0) return;
  const req: ActionRequest = { kind, targets, action, replicas, typed: "" };
  if (needsDialog(action, targets)) {
    actionUi.request = req;
    actionUi.busy = false;
    actionUi.error = "";
    actionUi.failures = [];
    actionUi.deleted = 0;
    actionUi.drain = null;
  } else {
    void execute(req, false);
  }
}

export function closeDialog() {
  const d = actionUi.drain;
  if (d && !d.finished && d.id) void commands.cmdCancelDrain(d.id);
  actionUi.request = null;
  actionUi.drain = null;
  actionUi.error = "";
  actionUi.failures = [];
}

/** The dialog's confirm button is enabled. */
export function canConfirm(req: ActionRequest): boolean {
  if (actionUi.busy) return false;
  if (req.action.input === "replicas" && !(Number.isInteger(req.replicas) && req.replicas >= 0)) return false;
  if (req.action.type_to_confirm && req.typed !== req.targets[0].name) return false;
  return true;
}

export function confirmRequest() {
  const req = actionUi.request;
  if (req && canConfirm(req)) void execute(req, true);
}

async function execute(req: ActionRequest, inDialog: boolean) {
  const { kind, targets, action } = req;
  const resource = kind.resource;
  const replicas = action.input === "replicas" ? req.replicas : null;
  if (inDialog) {
    actionUi.busy = true;
    actionUi.error = "";
  }
  try {
    if (action.id === "drain") {
      await startDrain(targets[0].name);
      return;
    }
    if (targets.length > 1) {
      const results = await commands.cmdBulkDelete(
        resource,
        targets.map((t) => ({ namespace: t.namespace, name: t.name })),
      );
      const failed = results.filter((r) => r.error !== "");
      if (failed.length === 0) {
        setNotice(true, `Deleted ${results.length} ${kind.label.toLowerCase()}`);
        actionUi.request = null;
      } else {
        actionUi.failures = failed;
        actionUi.deleted = results.length - failed.length;
      }
      actionUi.busy = false;
      return;
    }
    const t = targets[0];
    const msg = await commands.cmdRunAction(resource, t.namespace, t.name, action.id, replicas);
    if (action.id === "delete" || action.id === "forceDelete") closeDetailIf(kind, t);
    actionUi.request = null;
    actionUi.busy = false;
    setNotice(true, msg);
  } catch (e) {
    const text = String(e);
    actionUi.busy = false;
    if (inDialog) actionUi.error = text;
    else setNotice(false, `${action.label.replace("…", "")} ${targetLabel(targets[0])}: ${text}`);
  }
}

function closeDetailIf(kind: ResourceKind, t: ActionTarget) {
  const d = detail.target;
  if (d && sameResource(d.resource, kind.resource) && d.namespace === t.namespace && d.name === t.name) {
    closeDetail();
  }
}

async function startDrain(node: string) {
  actionUi.drain = { id: null, node, lines: [], finished: false, error: "", summary: null };
  try {
    const id = await commands.cmdDrainNode(node, DRAIN_TIMEOUT_SECS);
    if (actionUi.drain && actionUi.drain.id === null) actionUi.drain.id = id;
  } catch (e) {
    actionUi.drain = null;
    throw e;
  } finally {
    actionUi.busy = false;
  }
}

/** Subscribe to drain events; returns a cleanup function. */
export async function listenDrain(): Promise<() => void> {
  const un = await Promise.all([
    events.drainProgress.listen((e) => {
      const d = actionUi.drain;
      // The id is only known once cmd_drain_node answers, and the first event
      // may come before. Only one drain runs from the dialog at a time.
      if (!d || (d.id && d.id !== e.payload.drainId)) return;
      d.id ??= e.payload.drainId;
      d.lines.push(drainLine(e.payload.event));
    }),
    events.drainFinished.listen((e) => {
      const d = actionUi.drain;
      if (!d || (d.id && d.id !== e.payload.drainId)) return;
      d.id ??= e.payload.drainId;
      d.finished = true;
      d.error = e.payload.error;
      d.summary = e.payload.error ? null : e.payload.summary;
    }),
  ]);
  return () => un.forEach((f) => f());
}
