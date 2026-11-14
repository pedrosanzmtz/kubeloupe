// Pure logic of the debug dialog (ephemeral debug containers) and the node
// shell. The store is `stores/debug.svelte.ts`.

import type { ContainerDetail } from "./bindings";

/** Same default as the backend (`kubeloupe::debug::DEFAULT_DEBUG_IMAGE`). */
export const DEFAULT_DEBUG_IMAGE = "busybox:1.37";
/** Offered in the image field. */
export const DEBUG_IMAGES = [DEFAULT_DEBUG_IMAGE, "nicolaka/netshoot", "alpine:3.22"];
/** Where node shell pods run (`kubeloupe::node_shell::NODE_SHELL_NAMESPACE`). */
export const NODE_SHELL_NAMESPACE = "kube-system";

export const EPHEMERAL_NOTE =
  "Ephemeral containers cannot be removed: the debug container stays in the pod until the pod is deleted.";

/** Containers a debug container can target: the regular ones. */
export function debugTargets(containers: ContainerDetail[]): string[] {
  return containers.filter((c) => c.kind === "regular").map((c) => c.name);
}

/** An image reference with no spaces, or empty (the default). */
export function validImage(image: string): boolean {
  return !/\s/.test(image.trim());
}

/** The start-up offer to delete node shell pods left behind. */
export function leftoverText(count: number): string {
  return count === 1
    ? "A node shell pod from an earlier session is still running."
    : `${count} node shell pods from earlier sessions are still running.`;
}
