// The Debug dialog (pick a container and an image, then open a debug
// container in a dock tab), node shells, and the start-up offer to delete
// node shell pods left behind.

import { commands } from "../bindings";
import type { NodeShellPod } from "../bindings";
import { DEFAULT_DEBUG_IMAGE, NODE_SHELL_NAMESPACE, debugTargets, validImage } from "../debug";
import type { PodTabRef } from "../dock";
import { openDockTab } from "./dock.svelte";
import { setNotice } from "./actions.svelte";

export const debugDialog = $state({
  pod: null as PodTabRef | null,
  /** The pod's regular containers; empty while loading. */
  containers: [] as string[],
  target: "",
  image: DEFAULT_DEBUG_IMAGE,
  /** Looking up the containers; the target is not known yet. */
  loading: false,
});

/** Bumped per opening, so a late container lookup cannot fill a newer dialog. */
let opened = 0;

export function openDebugDialog(pod: PodTabRef) {
  const seq = ++opened;
  debugDialog.pod = pod;
  debugDialog.containers = [];
  debugDialog.target = "";
  debugDialog.image = DEFAULT_DEBUG_IMAGE;
  debugDialog.loading = true;
  commands
    .cmdGetResourceDetail(pod.resource, pod.namespace, pod.name)
    .then((d) => {
      if (seq !== opened || !debugDialog.pod) return;
      debugDialog.containers = debugTargets(d.pod.containers);
      debugDialog.target ||= debugDialog.containers[0] ?? "";
    })
    .catch(() => {
      // The backend falls back to the first container and reports errors.
    })
    .finally(() => {
      if (seq === opened) debugDialog.loading = false;
    });
}

export function closeDebugDialog() {
  debugDialog.pod = null;
}

export function canStartDebug(): boolean {
  return debugDialog.pod !== null && !debugDialog.loading && validImage(debugDialog.image);
}

/** Open the debug tab; the terminal adds the container and attaches. */
export function confirmDebugDialog() {
  const pod = debugDialog.pod;
  if (!pod || !canStartDebug()) return;
  openDockTab({
    type: "debug",
    pod,
    target: debugDialog.target,
    image: debugDialog.image.trim() || DEFAULT_DEBUG_IMAGE,
  });
  debugDialog.pod = null;
}

export function openNodeShell(node: string) {
  openDockTab({ type: "nodeShell", node, namespace: NODE_SHELL_NAMESPACE });
}

/** Node shell pods found at start-up (or after a context switch). */
export const leftovers = $state({ pods: [] as NodeShellPod[], busy: false });

export async function checkLeftovers() {
  try {
    leftovers.pods = await commands.cmdNodeShellLeftovers();
  } catch {
    leftovers.pods = [];
  }
}

export async function cleanUpLeftovers() {
  leftovers.busy = true;
  try {
    const n = await commands.cmdNodeShellCleanup();
    setNotice(true, n === 1 ? "Deleted 1 node shell pod" : `Deleted ${n} node shell pods`);
  } catch (e) {
    setNotice(false, `Could not delete the node shell pods: ${e}`);
  } finally {
    leftovers.busy = false;
    leftovers.pods = [];
  }
}

export function dismissLeftovers() {
  leftovers.pods = [];
}
